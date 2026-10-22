//! Settings, diagnostics and shell-integration commands.

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::core::config::Settings;
use crate::error::Result;
use crate::state::AppState;

#[tauri::command]
pub async fn settings_get(state: State<'_, AppState>) -> Result<Settings> {
    Ok(state.settings.read().await.clone())
}

#[tauri::command]
pub async fn settings_save(state: State<'_, AppState>, settings: Settings) -> Result<Settings> {
    {
        let mut current = state.settings.write().await;
        *current = settings.clone();
    }
    settings.save(&state.paths)?;
    Ok(settings)
}

/// Environment + path information shown on the Settings/About screen.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub os: String,
    pub arch: String,
    pub data_dir: String,
    pub instances_dir: String,
    pub java_dir: String,
    pub cache_dir: String,
    pub modrinth_api_url: String,
}

#[tauri::command]
pub async fn app_info(state: State<'_, AppState>) -> Result<AppInfo> {
    Ok(AppInfo {
        name: "MineArcade".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        data_dir: state.paths.root.to_string_lossy().to_string(),
        instances_dir: state.paths.instances_dir().to_string_lossy().to_string(),
        java_dir: state.paths.java_dir().to_string_lossy().to_string(),
        cache_dir: state.paths.cache_dir().to_string_lossy().to_string(),
        modrinth_api_url: state.modrinth_base().await,
    })
}

/// Reveal a file or folder in the OS file manager (Explorer / Finder / xdg-open).
#[tauri::command]
pub async fn system_reveal(app: AppHandle, path: String) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(&path, None::<&str>)
        .map_err(|e| crate::error::LauncherError::msg(e.to_string()))?;
    Ok(())
}

/// Open an arbitrary URL in the user's browser.
#[tauri::command]
pub async fn system_open_url(app: AppHandle, url: String) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(crate::error::LauncherError::msg("Only http:// and https:// URLs are allowed."));
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| crate::error::LauncherError::msg(e.to_string()))?;
    Ok(())
}

/// Free disk space for the drive that holds the launcher data, in bytes.
#[tauri::command]
pub async fn system_free_space(state: State<'_, AppState>) -> Result<Option<u64>> {
    // `fs2`-style statvfs isn't worth a dependency here: report `None` and let
    // the UI hide the widget on platforms we can't measure cheaply.
    let _ = state;
    Ok(None)
}
