//! In-game screenshots: listing them, and deleting one.

use tauri::State;

use crate::core::instance::game_dir;
use crate::core::screenshots::{self, Screenshot};
use crate::error::Result;
use crate::state::AppState;

/// An instance's screenshots, plus enough context for the UI to explain itself.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotsFile {
    /// `<game dir>/screenshots`, so the UI can reveal it.
    pub path: String,
    /// `false` when this instance has never taken a screenshot.
    pub exists: bool,
    pub screenshots: Vec<Screenshot>,
}

/// Every screenshot the game has taken in this instance, newest first.
#[tauri::command]
pub async fn screenshots_list(
    state: State<'_, AppState>,
    id: String,
) -> Result<ScreenshotsFile> {
    let instance = state.instances.read().await.get(&id)?.clone();
    let game = game_dir(&state.paths, &instance);
    let path = screenshots::dir(&game);

    Ok(ScreenshotsFile {
        exists: path.is_dir(),
        screenshots: screenshots::list(&game)?,
        path: path.to_string_lossy().to_string(),
    })
}

/// Delete one screenshot by file name.
#[tauri::command]
pub async fn screenshot_delete(
    state: State<'_, AppState>,
    id: String,
    name: String,
) -> Result<ScreenshotsFile> {
    let instance = state.instances.read().await.get(&id)?.clone();
    let game = game_dir(&state.paths, &instance);
    let path = screenshots::dir(&game);

    screenshots::remove(&game, &name)?;
    Ok(ScreenshotsFile {
        exists: path.is_dir(),
        screenshots: screenshots::list(&game)?,
        path: path.to_string_lossy().to_string(),
    })
}
