//! Shared application state handed to every Tauri command.

use std::sync::Arc;

use tokio::sync::RwLock;

use crate::core::auth::AccountStore;
use crate::core::config::Settings;
use crate::core::download::ProgressSink;
use crate::core::instance::InstanceStore;
use crate::core::launch::{GameEvent, GameEventSink};
use crate::core::paths::AppPaths;
use crate::error::Result;

/// One running game.
pub struct GameSession {
    pub instance_id: String,
    pub pid: u32,
    /// Sending `()` makes the watcher kill the game; taken once a stop was asked for.
    pub stop: Option<tokio::sync::oneshot::Sender<()>>,
}

pub struct AppState {
    /// Shared HTTP client with sane timeouts and a descriptive user agent.
    pub http: reqwest::Client,
    /// Where everything lives on disk.
    pub paths: AppPaths,
    pub settings: RwLock<Settings>,
    pub instances: RwLock<InstanceStore>,
    pub accounts: RwLock<AccountStore>,
    /// Held while a Microsoft token is being refreshed, so two commands needing
    /// a fresh token at once don't both redeem the same refresh token.
    pub token_refresh: tokio::sync::Mutex<()>,
    /// Every running game process, by session id. One instance can run several
    /// times at once (multi-launching), so a session, not an instance, is a game.
    pub sessions: tokio::sync::Mutex<std::collections::HashMap<u64, GameSession>>,
    /// The id the next launch gets.
    pub next_session: std::sync::atomic::AtomicU64,
    /// Discord Rich Presence (a no-op without an application ID).
    pub presence: crate::core::discord::Presence,
    /// Friends through Orbit Client's server: where it is, and the sign-in.
    pub social: crate::core::social::Social,
}

impl AppState {
    pub fn new(paths: AppPaths) -> Result<Self> {
        paths.ensure()?;

        let settings = Settings::load(&paths)?;
        let mut instances = InstanceStore::load(&paths)?;
        instances.reconcile(&paths)?;
        let accounts = AccountStore::load(&paths)?;

        let http = reqwest::Client::builder()
            .user_agent(crate::USER_AGENT)
            .timeout(std::time::Duration::from_secs(120))
            .connect_timeout(std::time::Duration::from_secs(20))
            .pool_max_idle_per_host(16)
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .map_err(crate::error::LauncherError::from)?;

        let presence = crate::core::discord::Presence::start(settings.discord_rich_presence);

        Ok(Self {
            http,
            paths,
            settings: RwLock::new(settings),
            instances: RwLock::new(instances),
            accounts: RwLock::new(accounts),
            token_refresh: tokio::sync::Mutex::new(()),
            sessions: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            next_session: std::sync::atomic::AtomicU64::new(1),
            presence,
            social: Default::default(),
        })
    }

    /// The instance id of every running game, once per game (an instance running twice is listed twice).
    pub async fn running_games(&self) -> Vec<String> {
        let sessions = self.sessions.lock().await;
        let mut ids: Vec<(u64, String)> = sessions
            .iter()
            .filter(|(_, game)| game.pid != 0)
            .map(|(session, game)| (*session, game.instance_id.clone()))
            .collect();
        ids.sort();
        ids.into_iter().map(|(_, id)| id).collect()
    }

    /// Whether any game of this instance is running (or starting).
    pub async fn is_running(&self, instance_id: &str) -> bool {
        self.sessions.lock().await.values().any(|game| game.instance_id == instance_id)
    }

    /// Current Modrinth base url, honouring user settings.
    pub async fn modrinth_base(&self) -> String {
        self.settings.read().await.modrinth_api_url.clone()
    }

    /// Put the featured server (farms.minearcade.org by default) at the top of a
    /// new instance's in-game multiplayer list. Best effort: a list that can't
    /// be written never stops an instance from being created.
    pub async fn seed_server_list(&self, game_dir: &std::path::Path) {
        let address = self.settings.read().await.featured_server.clone();
        if let Err(error) = crate::core::servers::ensure_server(
            game_dir,
            crate::core::config::DEFAULT_SERVER_NAME,
            &address,
        ) {
            log::warn!("could not add {address} to the server list: {error}");
        }
    }

    /// Add the play time of games a previous launcher run started but never saw exit
    /// (see [`crate::core::launch::RunningGame`]). One still running is checked again every
    /// 30 seconds until it's gone; the time is only credited then.
    pub fn adopt_orphaned_games(&self, app: &tauri::AppHandle) {
        use crate::core::launch;
        let mut orphans = launch::load_running(&self.paths);
        if orphans.is_empty() {
            return;
        }
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            use tauri::Manager;
            let state = app.state::<AppState>();
            loop {
                let mut still_running = Vec::new();
                for game in orphans {
                    if launch::java_is_running(game.pid).await {
                        still_running.push(game);
                        continue;
                    }
                    let mut store = state.instances.write().await;
                    if let Ok(instance) = store.get_mut(&game.instance_id) {
                        let game_dir = crate::core::instance::game_dir(&state.paths, instance);
                        let secs = launch::orphan_play_secs(&game, &game_dir);
                        log::info!("adding {secs}s of play time to {} from a game the launcher didn't see exit", game.instance_id);
                        instance.play_time_secs = instance.play_time_secs.saturating_add(secs);
                        if let Err(error) = store.persist(&state.paths, &game.instance_id) {
                            log::warn!("couldn't save play time for {}: {error}", game.instance_id);
                        }
                    }
                    launch::forget_running(&state.paths, &game);
                }
                if still_running.is_empty() {
                    break;
                }
                orphans = still_running;
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        });
    }

    /// A progress sink that forwards snapshots to the frontend.
    pub fn sink(&self, app: &tauri::AppHandle) -> ProgressSink {
        use tauri::Emitter;
        let app = app.clone();
        Arc::new(move |progress| {
            let _ = app.emit("download://progress", progress);
        })
    }

    /// A sink for game console output and process lifecycle events.
    ///
    /// Output goes out as `game://output` and the final status as `game://exit`,
    /// which is what the in-app console listens to. On exit the game's session is
    /// dropped and its length added to the instance's play time (and saved) first, so
    /// the UI's refresh on `game://exit` already shows both.
    pub fn game_sink(&self, app: &tauri::AppHandle) -> GameEventSink {
        use tauri::Emitter;
        let app = app.clone();
        Arc::new(move |event| match event {
            GameEvent::Output(output) => {
                let _ = app.emit("game://output", output);
            }
            GameEvent::Exit(exit) => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    use tauri::Manager;
                    let state = app.state::<AppState>();
                    {
                        let mut sessions = state.sessions.lock().await;
                        sessions.remove(&exit.session);
                        if !sessions.values().any(|game| game.instance_id == exit.instance_id) {
                            state.presence.game_stopped(&exit.instance_id);
                        }
                    }
                    {
                        let mut store = state.instances.write().await;
                        if let Ok(instance) = store.get_mut(&exit.instance_id) {
                            instance.play_time_secs = instance.play_time_secs.saturating_add(exit.play_secs);
                            instance.last_played = Some(chrono::Utc::now().to_rfc3339());
                            if let Err(error) = store.persist(&state.paths, &exit.instance_id) {
                                log::warn!("couldn't save play time for {}: {error}", exit.instance_id);
                            }
                        }
                    }
                    let _ = app.emit("game://exit", exit);
                });
            }
        })
    }
}
