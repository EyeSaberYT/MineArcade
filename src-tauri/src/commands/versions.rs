//! Minecraft version + loader version listing commands.

use serde::Serialize;
use tauri::State;

use crate::core::instance::LoaderKind;
use crate::core::minecraft::{self, loader, manifest::VersionSummary};
use crate::error::Result;
use crate::state::AppState;

/// A trimmed version entry for the UI's version picker.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionOption {
    pub id: String,
    pub version_type: String,
    pub release_time: String,
    /// True for the newest release the manifest advertises.
    pub latest_release: bool,
}

#[tauri::command]
pub async fn versions_list(
    state: State<'_, AppState>,
    include_snapshots: Option<bool>,
) -> Result<Vec<VersionOption>> {
    let include = include_snapshots.unwrap_or_else(|| false);

    // Prefer the cached manifest; refresh only when we have nothing.
    let manifest = minecraft::manifest::fetch_manifest(&state.http, &state.paths, false).await?;

    Ok(manifest
        .versions
        .iter()
        .filter(|v| include || v.version_type == "release")
        .map(|v| VersionOption {
            id: v.id.clone(),
            version_type: v.version_type.clone(),
            release_time: v.release_time.clone(),
            latest_release: false,
        })
        .map(|mut option| {
            option.latest_release = option.id == manifest.latest.release;
            option
        })
        .collect())
}

#[tauri::command]
pub async fn versions_refresh(state: State<'_, AppState>) -> Result<()> {
    minecraft::manifest::fetch_manifest(&state.http, &state.paths, true).await?;
    Ok(())
}

#[tauri::command]
pub async fn versions_latest(state: State<'_, AppState>) -> Result<String> {
    let manifest = minecraft::manifest::fetch_manifest(&state.http, &state.paths, false).await?;
    Ok(manifest.latest.release)
}

#[tauri::command]
pub async fn loader_versions(
    state: State<'_, AppState>,
    loader_kind: String,
    game_version: String,
) -> Result<Vec<loader::LoaderVersion>> {
    let kind = LoaderKind::parse(&loader_kind)
        .ok_or_else(|| crate::error::LauncherError::msg("Unknown mod loader."))?;
    loader::list_loader_versions(&state.http, kind, &game_version).await
}

/// Raw manifest entry lookup, handy for diagnostics.
#[tauri::command]
pub async fn version_details(
    state: State<'_, AppState>,
    id: String,
) -> Result<VersionSummary> {
    let manifest = minecraft::manifest::fetch_manifest(&state.http, &state.paths, false).await?;
    Ok(minecraft::manifest::find_version(&manifest, &id)?.clone())
}
