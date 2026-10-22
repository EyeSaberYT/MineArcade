//! Commands that discover and import instances from other launchers.
//!
//! MineArcade never moves or edits the other launcher's files: every import is a
//! copy into a brand-new instance, and the scan step only reads metadata off
//! disk. The command layer is responsible for turning that metadata into a real
//! instance (creating its directory, installing the loader, and persisting it).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::core::import::{self, ImportCandidate, ImportScan, ImportSource};
use crate::core::instance::{game_dir, Instance, LoaderKind};
use crate::core::{java, minecraft};
use crate::error::{LauncherError, Result};
use crate::state::AppState;

/// Payload the import dialog sends back for each instance it wants MineArcade to
/// copy in.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRequest {
    /// The launcher the folder came from; used to pick the correct skip list.
    pub source: ImportSource,
    /// Name to give the new MineArcade instance.
    pub name: String,
    /// Game folder to copy from.
    pub path: String,
    /// Minecraft version to install for the imported instance.
    pub game_version: String,
    /// Loader the instance uses.
    #[serde(default)]
    pub loader: LoaderKind,
    /// The exact loader build, when the source had one pinned.
    pub loader_version: Option<String>,
    /// Absolute path of an icon image to copy into the new instance, when one
    /// was discovered in the source launcher.
    pub icon: Option<String>,
}

/// What happened to a single requested import.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFailure {
    pub name: String,
    pub path: String,
    pub error: String,
}

/// The outcome of a bulk import: some may have succeeded, others failed.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    pub imported: Vec<Instance>,
    pub failed: Vec<ImportFailure>,
}

/// Discover all importable instances the user already has on disk.
///
/// This is cheap (mostly directory walks + a small SQLite read), and it runs
/// in the background via `spawn_blocking`. Nothing moves or edits: only
/// metadata is read.
#[tauri::command]
pub async fn import_scan(state: State<'_, AppState>) -> Result<ImportScan> {
    let result = tokio::task::spawn_blocking(import::scan).await;
    let mut scan = result.map_err(|_| LauncherError::msg("The scan was cancelled."))?;

    // Resolve `latest-release` / `latest-snapshot` placeholders against the
    // cached version manifest. Failing to resolve is not fatal: any candidate
    // that remains unresolved (or whose version is missing) simply shows up as
    // needing a choice in the UI.
    if scan
        .candidates
        .iter()
        .any(|candidate| candidate.wants_latest().is_some())
    {
        match minecraft::manifest::fetch_manifest(&state.http, &state.paths, false).await {
            Ok(manifest) => {
                for candidate in &mut scan.candidates {
                    match candidate.wants_latest() {
                        Some(true) if !manifest.latest.release.is_empty() => {
                            candidate.game_version = Some(manifest.latest.release.clone());
                        }
                        Some(false) if !manifest.latest.snapshot.is_empty() => {
                            candidate.game_version = Some(manifest.latest.snapshot.clone());
                        }
                        _ => {}
                    }
                }
            }
            Err(error) => {
                // Don't abort the scan: the UI will ask the user to choose a
                // version for candidates that cannot be resolved automatically.
                log::warn!("import: could not resolve latest versions: {error}");
            }
        }
    }

    Ok(scan)
}

/// Inspect a single folder the user picked by hand and return the candidate(s)
/// inside it. The folder might be a launcher's data directory, an instance
/// folder, or the raw `.minecraft` itself.
#[tauri::command]
pub async fn import_inspect(path: String) -> Result<Vec<ImportCandidate>> {
    let path_buf = PathBuf::from(&path);
    let result = tokio::task::spawn_blocking(move || import::inspect(&path_buf)).await;
    result.map_err(|_| LauncherError::msg("Inspection was cancelled."))?
}

/// Copy the requested game folders into brand-new MineArcade instances.
///
/// Each successful import creates an instance (persisted in the store), copies
/// the player's files in, and installs the loader the source specified. When a
/// loader build cannot be installed, the import fails for that item and the
/// copied files are deleted (nothing of the user's originals is touched).
#[tauri::command]
pub async fn import_instances(
    app: AppHandle,
    state: State<'_, AppState>,
    requests: Vec<ImportRequest>,
) -> Result<ImportOutcome> {
    let mut imported = Vec::new();
    let mut failed = Vec::new();

    for request in requests {
        match import_one(&app, &state, request.clone()).await {
            Ok(instance) => imported.push(instance),
            Err(error) => failed.push(ImportFailure {
                name: request.name,
                path: request.path,
                error: error.to_string(),
            }),
        }
    }

    Ok(ImportOutcome { imported, failed })
}

/// Import a single request, returning the newly created instance on success.
/// On failure, any files that were copied are rolled back.
async fn import_one(
    app: &AppHandle,
    state: &State<'_, AppState>,
    request: ImportRequest,
) -> Result<Instance> {
    let name = request.name.trim();
    if name.is_empty() {
        return Err(LauncherError::msg("Give your instance a name."));
    }

    // The game folder must exist and be readable.
    let from = PathBuf::from(&request.path);
    if !from.is_dir() {
        return Err(LauncherError::msg("The chosen game folder does not exist."));
    }

    // Create a blank instance with the version and loader the user chose.
    let mut instance = Instance::new(name, &request.game_version, request.loader);
    instance.icon = None;
    instance.group = None;
    instance.tags = vec![];
    instance.loader_version = request.loader_version.clone();

    // Inherit global defaults so the new instance feels consistent.
    {
        let settings = state.settings.read().await;
        instance.settings.memory_min = settings.default_memory_min;
        instance.settings.memory_max = settings.default_memory_max;
    }

    let id = instance.id.clone();
    let instance_dir = state.paths.instance_dir(&id);
    let game_dir_path = game_dir(&state.paths, &instance);

    // Create the instance directory structure before copying.
    std::fs::create_dir_all(&game_dir_path)?;
    for sub in ["mods", "resourcepacks", "shaderpacks", "saves", "logs"] {
        std::fs::create_dir_all(game_dir_path.join(sub))?;
    }

    // Apply the chosen icon. A path to an image is copied into the instance so
    // it keeps working after the import; an emoji or short label is stored as-is.
    instance.icon = crate::commands::instance::resolve_icon(
        request.icon,
        &instance_dir.join("icon.png"),
    );

    // Copy the player's game files into the new instance.
    import::copy_game_folder(&from, &game_dir_path, request.source, state.sink(app))
        .inspect_err(|_| {
            // Roll back: remove the instance folder we just created. Any icon
            // copied into it goes with it.
            let _ = std::fs::remove_dir_all(&instance_dir);
        })?;

    // Install the loader. The latest stable is used when the source did not pin
    // a specific build. For Fabric/Quilt the install is lightweight; for
    // Forge/NeoForge MineArcade may download the installer and (if needed) Java 17.
    if instance.loader != LoaderKind::Vanilla {
        let loader_version = match instance.loader_version.clone() {
            Some(version) => version,
            None => {
                let versions = minecraft::loader::list_loader_versions(
                    &state.http,
                    instance.loader,
                    &instance.game_version,
                )
                .await?;

                versions
                    .iter()
                    .find(|version| version.stable)
                    .or_else(|| versions.first())
                    .map(|version| version.version.clone())
                    .ok_or_else(|| {
                        LauncherError::msg(format!(
                            "No {} build is available for Minecraft {}.",
                            instance.loader.as_str(),
                            instance.game_version,
                        ))
                    })?
            }
        };

        // Forge-family installers require Java 17. Detect it (or download it) so
        // the install can proceed. Fabric/Quilt do not need an explicit path.
        let java_path = match instance.loader {
            LoaderKind::Forge | LoaderKind::NeoForge => {
                match java::detect(&state.paths, None, 17).await {
                    Some(runtime) => runtime.as_path(),
                    None => {
                        let runtime = java::install(&state.http, &state.paths, 17, state.sink(app)).await?;
                        runtime.as_path()
                    }
                }
            }
            _ => PathBuf::from("java"),
        };

        instance.loader_version = Some(loader_version.clone());

        if let Err(error) = minecraft::loader::install_loader(
            &state.http,
            &state.paths,
            &game_dir_path,
            instance.loader,
            &instance.game_version,
            &loader_version,
            &java_path,
            state.sink(app),
        )
        .await
        {
            // Roll back the entire imported instance on loader install failure.
            let _ = std::fs::remove_dir_all(&instance_dir);
            return Err(LauncherError::msg(format!(
                "Could not set up {} for this instance: {error}",
                instance.loader.as_str()
            )));
        }
    }

    // Persist the new instance.
    state
        .seed_server_list(&crate::core::instance::game_dir(&state.paths, &instance))
        .await;

    {
        let mut store = state.instances.write().await;
        store.instances.push(instance.clone());
        store.persist(&state.paths, &instance.id)?;
    }

    Ok(instance)
}
