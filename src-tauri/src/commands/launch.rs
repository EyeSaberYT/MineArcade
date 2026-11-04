//! Launch + log commands.

use tauri::{AppHandle, State};

use crate::commands::auth::active_account;
use crate::core::instance::QuickPlay;
use crate::core::launch::{self, LaunchRequest, LaunchedProcess};
use crate::error::{LauncherError, Result};
use crate::state::{AppState, GameSession};

/// Launch an instance. Emits `download://progress` events throughout.
///
/// An instance that's already running can be launched again (multi-launching):
/// each launch is its own game, with its own window and process.
///
/// `quick_play` overrides where the game opens for this launch only. The server
/// list uses it to join a chosen server without rewriting the instance's saved
/// settings, which is what a "join once" action should mean.
#[tauri::command]
pub async fn launch_instance(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    quick_play: Option<QuickPlay>,
) -> Result<LaunchedProcess> {
    // Orbit Client plays its newest release, and every other Fabric instance gets it too.
    // Not while a game of this instance runs: it holds the mod jars open, so they can't be
    // replaced, and the running copy already has them.
    if state.is_running(&id).await {
        log::info!("instance {id} is already running: launching another copy as it is");
    } else if crate::commands::orbit::is_orbit(state.instances.read().await.get(&id)?) {
        crate::commands::orbit::prepare_launch(&app, &state, &id).await?;
    } else {
        crate::commands::orbit::prepare_other_launch(&app, &state, &id).await;
    }
    let mut instance = state.instances.read().await.get(&id)?.clone();
    if let Some(quick_play) = quick_play {
        instance.settings.quick_play = quick_play;
    }

    let account = active_account(&state).await?;
    let settings = state.settings.read().await.clone();

    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let session = state.next_session.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // Registered before the game starts, so a game that exits at once (its exit drops the
    // session) can't leave a session behind. It counts as running once it has a pid.
    state.sessions.lock().await.insert(
        session,
        GameSession { instance_id: id.clone(), pid: 0, stop: Some(stop_tx) },
    );
    let launched = launch::launch(LaunchRequest {
        http: &state.http,
        paths: &state.paths,
        instance: &instance,
        account: &account,
        settings: &settings,
        sink: state.sink(&app),
        events: state.game_sink(&app),
        dry_run: false,
        stop: Some(stop_rx),
        session,
    })
    .await;
    let result = match launched {
        Ok(result) => result,
        Err(error) => {
            state.sessions.lock().await.remove(&session);
            return Err(error);
        }
    };

    // Running until the process exits (AppState::game_sink drops the session then). A game
    // that already exited is gone from the map, and its exit already cleared the presence:
    // checked under the lock so the exit can't slip in between.
    {
        let mut sessions = state.sessions.lock().await;
        if let Some(game) = sessions.get_mut(&session) {
            game.pid = result.pid;
            state.presence.game_started(crate::core::discord::Game::from_instance(&instance));
        }
    }
    {
        let mut store = state.instances.write().await;
        if let Ok(entry) = store.get_mut(&id) {
            entry.last_played = Some(chrono::Utc::now().to_rfc3339());
        }
        let _ = store.save(&state.paths);
    }

    Ok(result)
}

/// Return the exact command that would be used, without starting the game.
#[tauri::command]
pub async fn launch_preview(state: State<'_, AppState>, id: String) -> Result<String> {
    let instance = state.instances.read().await.get(&id)?.clone();
    let account = active_account(&state).await?;
    let settings = state.settings.read().await.clone();

    let result = launch::launch(LaunchRequest {
        http: &state.http,
        paths: &state.paths,
        instance: &instance,
        account: &account,
        settings: &settings,
        sink: crate::core::download::silent(),
        events: launch::silent_events(),
        dry_run: true,
        stop: None,
        session: 0,
    })
    .await?;

    let quoted = result
        .arguments
        .iter()
        .map(|a| {
            let mut s = a.clone();
            // Redact any occurrence of tokens.
            if !account.access_token.is_empty() && s.contains(&account.access_token) {
                s = s.replace(&account.access_token, "****");
            }
            if let Some(rt) = &account.refresh_token {
                if !rt.is_empty() && s.contains(rt) {
                    s = s.replace(rt, "****");
                }
            }
            if let Some(idx) = s.find("token:") {
                let rest = &s[idx + 6..];
                if !rest.is_empty() && rest.len() > 4 {
                    s.replace_range(idx + 6.., "****");
                } else {
                    s = s.replace(&s[idx..], "token:****");
                }
            }
            if s.contains(' ') { format!("\"{s}\"") } else { s }
        })
        .collect::<Vec<_>>()
        .join(" ");
    Ok(format!("{} {}", result.command, quoted))
}

/// Read the tail of an instance's most recent launch log.
#[tauri::command]
pub async fn launch_logs(
    state: State<'_, AppState>,
    id: String,
    lines: Option<usize>,
) -> Result<Option<String>> {
    launch::read_latest_log(&state.paths, &id, lines.unwrap_or(400))
}

/// Mark an instance as no longer running (used when the launcher loses track).
#[tauri::command]
pub async fn launch_mark_stopped(state: State<'_, AppState>, id: String) -> Result<()> {
    // Copies still starting (no pid yet) are tracked by their launch, not lost.
    state.sessions.lock().await.retain(|_, game| game.instance_id != id || game.pid == 0);
    state.presence.game_stopped(&id);
    Ok(())
}

/// Force-close every game of an instance, including copies still starting (their launch
/// is called off, or the game killed as soon as it's spawned). `game://exit` follows for
/// each running one once its process is gone.
#[tauri::command]
pub async fn launch_stop(state: State<'_, AppState>, id: String) -> Result<()> {
    let mut stopped = 0;
    let mut kill = Vec::new();
    {
        let mut sessions = state.sessions.lock().await;
        for (session, game) in sessions.iter_mut().filter(|(_, game)| game.instance_id == id) {
            stopped += 1;
            log::info!("stop requested for instance {id} (session {session}, pid {})", game.pid);
            // The watcher kills the game when told to. Already asked once: kill the process
            // directly so Stop always does something. A closed channel means the watcher is
            // done, so the pid may belong to another program by now: leave it alone.
            match game.stop.take() {
                Some(stop) => {
                    let _ = stop.send(());
                }
                None if game.pid != 0 => kill.push(game.pid),
                None => {}
            }
        }
    }
    if stopped == 0 {
        return Err(LauncherError::msg("That instance isn't running."));
    }
    // taskkill can take a while: not under the sessions lock.
    for pid in kill {
        launch::kill_tree(pid).await;
    }
    Ok(())
}

/// The instance id of every running game: an instance running twice is listed twice.
#[tauri::command]
pub async fn launch_running(state: State<'_, AppState>) -> Result<Vec<String>> {
    Ok(state.running_games().await)
}
