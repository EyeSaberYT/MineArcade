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

pub struct AppState {
    /// Shared HTTP client with sane timeouts and a descriptive user agent.
    pub http: reqwest::Client,
    /// Where everything lives on disk.
    pub paths: AppPaths,
    pub settings: RwLock<Settings>,
    pub instances: RwLock<InstanceStore>,
    pub accounts: RwLock<AccountStore>,
    /// Instances that currently have a game process running.
    pub running: RwLock<Vec<String>>,
    /// Per running instance, the channel that kills its game process.
    pub stoppers: tokio::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<()>>>,
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

        Ok(Self {
            http,
            paths,
            settings: RwLock::new(settings),
            instances: RwLock::new(instances),
            accounts: RwLock::new(accounts),
            running: RwLock::new(vec![]),
            stoppers: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// Current Modrinth base url, honouring user settings.
    pub async fn modrinth_base(&self) -> String {
        self.settings.read().await.modrinth_api_url.clone()
    }

    /// Put the featured server (play.minearcade.org by default) at the top of a
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
    /// which is what the in-app console listens to.
    pub fn game_sink(&self, app: &tauri::AppHandle) -> GameEventSink {
        use tauri::Emitter;
        let app = app.clone();
        Arc::new(move |event| match event {
            GameEvent::Output(output) => {
                let _ = app.emit("game://output", output);
            }
            GameEvent::Exit(exit) => {
                let _ = app.emit("game://exit", exit);
            }
        })
    }
}
