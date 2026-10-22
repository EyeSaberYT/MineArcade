//! World backups: what an instance has, and zipping one back up.

use std::path::{Path, PathBuf};

use tauri::State;

use crate::core::backups::{self, Backup, Restored, World};
use crate::core::instance::game_dir;
use crate::error::{LauncherError, Result};
use crate::state::AppState;

/// Everything the Worlds tab needs, in one round trip.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldsOverview {
    /// `<game dir>/saves`, so the UI can reveal it.
    pub saves_path: String,
    /// Where MineArcade keeps its zips, so the UI can reveal that too.
    pub backups_path: String,
    pub worlds: Vec<World>,
    pub backups: Vec<Backup>,
}

/// The two directories a world operation touches, resolved from an instance id.
async fn dirs(state: &State<'_, AppState>, id: &str) -> Result<(PathBuf, PathBuf)> {
    let instance = state.instances.read().await.get(id)?.clone();
    Ok((
        game_dir(&state.paths, &instance),
        state.paths.instance_dir(id),
    ))
}

fn overview(game: &Path, instance_dir: &Path) -> Result<WorldsOverview> {
    Ok(WorldsOverview {
        saves_path: backups::saves_dir(game).to_string_lossy().to_string(),
        backups_path: backups::backups_dir(instance_dir)
            .to_string_lossy()
            .to_string(),
        worlds: backups::worlds(game)?,
        backups: backups::list(instance_dir)?,
    })
}

/// Refuse a world operation while the game is holding its worlds open.
///
/// The game rewrites a world as it plays and again on exit, so zipping one while
/// it runs can capture a half-written chunk and replacing one would simply be
/// overwritten when the game saves next.
async fn ensure_stopped(state: &State<'_, AppState>, id: &str) -> Result<()> {
    if state.running.read().await.iter().any(|entry| entry == id) {
        return Err(LauncherError::msg(
            "Minecraft is playing this instance, so its worlds are being written. \
             Close the game first.",
        ));
    }
    Ok(())
}

/// Every world in the instance and every backup MineArcade has taken of them.
#[tauri::command]
pub async fn worlds_overview(state: State<'_, AppState>, id: String) -> Result<WorldsOverview> {
    let (game, instance_dir) = dirs(&state, &id).await?;
    overview(&game, &instance_dir)
}

/// Zip a world into the instance's backup folder.
#[tauri::command]
pub async fn world_backup_create(
    state: State<'_, AppState>,
    id: String,
    world: String,
) -> Result<WorldsOverview> {
    ensure_stopped(&state, &id).await?;
    let (game, instance_dir) = dirs(&state, &id).await?;

    // A large world takes seconds to deflate; keep it off the async runtime.
    tokio::task::spawn_blocking(move || {
        backups::create(&game, &instance_dir, &world)?;
        overview(&game, &instance_dir)
    })
    .await
    .map_err(|error| LauncherError::msg(format!("The backup could not run: {error}")))?
}

/// Replace a world with the contents of one of its backups.
#[tauri::command]
pub async fn world_backup_restore(
    state: State<'_, AppState>,
    id: String,
    file: String,
) -> Result<Restored> {
    ensure_stopped(&state, &id).await?;
    let (game, instance_dir) = dirs(&state, &id).await?;

    tokio::task::spawn_blocking(move || backups::restore(&game, &instance_dir, &file))
        .await
        .map_err(|error| LauncherError::msg(format!("The restore could not run: {error}")))?
}

/// Delete one of the instance's backups by file name.
#[tauri::command]
pub async fn world_backup_delete(
    state: State<'_, AppState>,
    id: String,
    file: String,
) -> Result<WorldsOverview> {
    let (game, instance_dir) = dirs(&state, &id).await?;

    // Deleting a zip the game never opens is safe at any time; it is only the
    // worlds themselves that need the game closed.
    tokio::task::spawn_blocking(move || {
        backups::remove(&instance_dir, &file)?;
        overview(&game, &instance_dir)
    })
    .await
    .map_err(|error| LauncherError::msg(format!("The backup could not be deleted: {error}")))?
}
