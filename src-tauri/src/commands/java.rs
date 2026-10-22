//! Java runtime commands.

use tauri::{AppHandle, State};

use crate::core::java::{self, JavaRuntime};
use crate::error::Result;
use crate::state::AppState;

/// Every Java runtime we can find on this machine.
#[tauri::command]
pub async fn java_list(state: State<'_, AppState>) -> Result<Vec<JavaRuntime>> {
    Ok(java::list(&state.paths).await)
}

/// A Java runtime that satisfies `required_major`, if one exists.
#[tauri::command]
pub async fn java_detect(
    state: State<'_, AppState>,
    required_major: Option<u32>,
) -> Result<Option<JavaRuntime>> {
    Ok(java::detect(&state.paths, None, required_major.unwrap_or(0)).await)
}

/// Download a Mojang-provided runtime for the requested major version.
#[tauri::command]
pub async fn java_install(
    app: AppHandle,
    state: State<'_, AppState>,
    required_major: u32,
) -> Result<JavaRuntime> {
    java::install(&state.http, &state.paths, required_major, state.sink(&app)).await
}

/// Java major version a specific Minecraft version needs.
#[tauri::command]
pub async fn java_requirement(
    state: State<'_, AppState>,
    game_version: String,
) -> Result<u32> {
    let manifest =
        crate::core::minecraft::manifest::fetch_manifest(&state.http, &state.paths, false).await?;
    let summary = crate::core::minecraft::manifest::find_version(&manifest, &game_version)?;
    let version =
        crate::core::minecraft::manifest::fetch_version_json(&state.http, &state.paths, summary)
            .await?;
    Ok(version
        .java_version
        .map(|j| java::required_major(&j.component, j.major_version))
        .unwrap_or(8))
}
