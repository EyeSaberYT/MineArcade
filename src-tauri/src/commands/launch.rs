//! Launch + log commands.

use tauri::{AppHandle, State};

use crate::commands::auth::active_account;
use crate::core::instance::QuickPlay;
use crate::core::launch::{self, LaunchRequest, LaunchedProcess};
use crate::error::{LauncherError, Result};
use crate::state::AppState;

/// Launch an instance. Emits `download://progress` events throughout.
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
    // Make sure the loader profile is installed before we try to resolve it.
    let mut instance = state.instances.read().await.get(&id)?.clone();
    if let Some(quick_play) = quick_play {
        instance.settings.quick_play = quick_play;
    }

    {
        let running = state.running.read().await;
        if running.contains(&id) {
            return Err(LauncherError::msg("That instance is already running."));
        }
    }

    let account = active_account(&state).await?;
    let settings = state.settings.read().await.clone();

    let result = launch::launch(LaunchRequest {
        http: &state.http,
        paths: &state.paths,
        instance: &instance,
        account: &account,
        settings: &settings,
        sink: state.sink(&app),
        events: state.game_sink(&app),
        dry_run: false,
    })
    .await?;

    // Track it as running until the process exits.
    state.running.write().await.push(id.clone());
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
    state.running.write().await.retain(|entry| entry != &id);
    Ok(())
}

/// Which instances currently have a game process.
#[tauri::command]
pub async fn launch_running(state: State<'_, AppState>) -> Result<Vec<String>> {
    Ok(state.running.read().await.clone())
}
