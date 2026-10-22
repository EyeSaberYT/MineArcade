//! Instance lifecycle + content (mods / packs) management commands.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::core::instance::{
    self, game_dir, slugify, Instance, InstanceSettings, InstalledProject, LoaderKind,
};
use crate::core::minecraft::loader;
use crate::core::modpack;
use crate::core::path_security::{canonical_path_is_within, is_safe_filename};
use crate::core::{java, launch};
use crate::error::{LauncherError, Result};
use crate::state::AppState;

/// Payload used when creating a new instance.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateInstanceArgs {
    pub name: String,
    pub game_version: String,
    #[serde(default)]
    pub loader: LoaderKind,
    pub loader_version: Option<String>,
    pub icon: Option<String>,
    pub group: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Install the loader profile immediately.
    #[serde(default)]
    pub install_loader: bool,
}

#[tauri::command]
pub async fn instance_list(state: State<'_, AppState>) -> Result<Vec<launch::InstanceSummary>> {
    let store = state.instances.read().await;
    Ok(store
        .instances
        .iter()
        .map(|i| launch::summarize(&state.paths, i))
        .collect())
}

#[tauri::command]
pub async fn instance_get(state: State<'_, AppState>, id: String) -> Result<Instance> {
    Ok(state.instances.read().await.get(&id)?.clone())
}

/// Resolve a user-chosen icon into something an instance can store.
///
/// An icon is either a short piece of text (an emoji or a couple of letters,
/// stored verbatim) or a path to an image file. Image files are copied into
/// the instance's own folder so the icon keeps working even if the original is
/// later moved or deleted; the copy's absolute path is what gets stored.
///
/// `target` is the file the image should be copied to (e.g. `<instance>/icon.png`).
/// A failed copy is not fatal: the original path is stored instead, which still
/// renders through the asset protocol for as long as the file exists.
pub(crate) fn resolve_icon(
    icon: Option<String>,
    target: &std::path::Path,
) -> Option<String> {
    let icon = icon?;
    let source = PathBuf::from(&icon);
    if !source.is_file() {
        // Not a file on disk: an emoji or short label, kept as-is.
        return Some(icon);
    }
    if let Some(parent) = target.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::copy(&source, target) {
        Ok(_) => Some(target.to_string_lossy().to_string()),
        Err(error) => {
            log::warn!("icon: could not copy {}: {error}", source.display());
            Some(icon)
        }
    }
}

#[tauri::command]
pub async fn instance_create(
    app: AppHandle,
    state: State<'_, AppState>,
    args: CreateInstanceArgs,
) -> Result<Instance> {
    let name = args.name.trim();
    if name.is_empty() {
        return Err(LauncherError::msg("Give your instance a name."));
    }

    let mut instance = Instance::new(name, &args.game_version, args.loader);
    instance.icon = resolve_icon(args.icon, &state.paths.instance_dir(&instance.id).join("icon.png"));
    instance.group = args.group;
    instance.tags = args.tags;
    instance.loader_version = args.loader_version.clone();

    // Inherit global defaults so new instances feel consistent.
    {
        let settings = state.settings.read().await;
        instance.settings.memory_min = settings.default_memory_min;
        instance.settings.memory_max = settings.default_memory_max;
    }

    let dir = game_dir(&state.paths, &instance);
    std::fs::create_dir_all(&dir)?;
    for sub in ["mods", "resourcepacks", "shaderpacks", "saves", "logs"] {
        std::fs::create_dir_all(dir.join(sub))?;
    }

    // Install the loader up front when requested.
    if args.install_loader && instance.loader != LoaderKind::Vanilla {
        let loader_version = match instance.loader_version.clone() {
            Some(version) => version,
            None => {
                let versions =
                    loader::list_loader_versions(&state.http, instance.loader, &instance.game_version)
                        .await?;
                versions
                    .iter()
                    .find(|v| v.stable)
                    .or_else(|| versions.first())
                    .map(|v| v.version.clone())
                    .ok_or_else(|| {
                        LauncherError::msg(format!(
                            "No {} build is available for Minecraft {}.",
                            instance.loader.as_str(),
                            instance.game_version
                        ))
                    })?
            }
        };

        // Forge-family installers need a Java runtime of their own.
        let java_path = match instance.loader {
            LoaderKind::Forge | LoaderKind::NeoForge => java::detect(&state.paths, None, 17)
                .await
                .map(|r| r.as_path())
                .ok_or_else(|| LauncherError::msg("Installing Forge requires Java 17 or newer."))?,
            _ => PathBuf::from("java"),
        };

        loader::install_loader(
            &state.http,
            &state.paths,
            &dir,
            instance.loader,
            &instance.game_version,
            &loader_version,
            &java_path,
            state.sink(&app),
        )
        .await?;

        instance.loader_version = Some(loader_version);
    }

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

#[tauri::command]
pub async fn instance_update(state: State<'_, AppState>, mut instance: Instance) -> Result<Instance> {
    // A custom image chosen on the settings page is copied into the instance
    // folder here, matching create/import. Once stored, the icon already points
    // at that copy, so resolving again is a no-op rather than a self-copy.
    if let Some(icon) = &instance.icon {
        if PathBuf::from(icon).is_file() {
            let target = state.paths.instance_dir(&instance.id).join("icon.png");
            if *icon != target {
                instance.icon = resolve_icon(Some(icon.clone()), &target);
            }
        }
    }

    let mut store = state.instances.write().await;
    *store.get_mut(&instance.id)? = instance.clone();
    store.persist(&state.paths, &instance.id)?;
    Ok(instance)
}

#[tauri::command]
pub async fn instance_rename(
    state: State<'_, AppState>,
    id: String,
    name: String,
) -> Result<Instance> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(LauncherError::msg("The instance name can't be empty."));
    }
    let mut store = state.instances.write().await;
    let instance = {
        let entry = store.get_mut(&id)?;
        entry.name = name;
        entry.clone()
    };
    store.persist(&state.paths, &id)?;
    Ok(instance)
}

#[tauri::command]
pub async fn instance_duplicate(
    state: State<'_, AppState>,
    id: String,
    name: Option<String>,
) -> Result<Instance> {
    let source = state.instances.read().await.get(&id)?.clone();

    let mut copy = source.clone();
    copy.id = instance::new_id();
    copy.name = name.unwrap_or_else(|| format!("{} (copy)", source.name));
    copy.created = chrono::Utc::now().to_rfc3339();
    copy.last_played = None;
    copy.play_time_secs = 0;

    // Recursively copy the game directory (minus caches, logs and metadata).
    copy_game_dir(
        &game_dir(&state.paths, &source),
        &state.paths.instance_game_dir(&copy.id),
    )?;

    // The game directory is the instance folder now, so the copy already has
    // its own `icon.png`; re-point the field at it instead of leaving the
    // duplicate referencing the source instance's folder.
    copy.icon = resolve_icon(
        copy.icon.clone(),
        &state.paths.instance_dir(&copy.id).join("icon.png"),
    );

    {
        let mut store = state.instances.write().await;
        store.instances.push(copy.clone());
        store.persist(&state.paths, &copy.id)?;
    }
    Ok(copy)
}

/// Copy a directory tree, skipping regenerable folders and per-instance
/// launcher files (the store rewrites `instance.json` right after).
fn copy_game_dir(from: &std::path::Path, to: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    let skip = ["logs", "crash-reports", "screenshots", ".cache", "instance.json"];
    for entry in walkdir::WalkDir::new(from)
        .into_iter()
        .filter_entry(|e| {
            !(e.depth() == 1 && skip.contains(&e.file_name().to_string_lossy().as_ref()))
        })
    {
        let entry = entry.map_err(|e| LauncherError::msg(e.to_string()))?;
        let relative = entry
            .path()
            .strip_prefix(from)
            .map_err(|_| LauncherError::msg("invalid path"))?;
        let target = to.join(relative);

        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn instance_delete(
    state: State<'_, AppState>,
    id: String,
    delete_files: bool,
) -> Result<()> {
    if id.is_empty()
        || id == "."
        || id == ".."
        || id.chars().any(|c| matches!(c, '/' | '\\' | ':'))
        || id.chars().any(char::is_control)
    {
        return Err(LauncherError::msg("Invalid instance id."));
    }

    let mut store = state.instances.write().await;
    store.get(&id)?;
    if delete_files {
        let dir = state.paths.instance_dir(&id);
        if dir.exists() {
            let instances_root = state.paths.instances_dir().canonicalize()?;
            let canonical_dir = dir.canonicalize()?;
            if canonical_dir.parent() != Some(instances_root.as_path()) {
                return Err(LauncherError::msg(
                    "The instance folder resolves outside the launcher data directory.",
                ));
            }
            std::fs::remove_dir_all(&dir)?;
        }
    }
    store.instances.retain(|i| i.id != id);
    store.save(&state.paths)?;
    Ok(())
}

/// Install (or reinstall) the loader for an existing instance.
#[tauri::command]
pub async fn instance_install_loader(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    loader_version: Option<String>,
) -> Result<Instance> {
    let (instance, dir) = {
        let store = state.instances.read().await;
        let instance = store.get(&id)?.clone();
        let dir = game_dir(&state.paths, &instance);
        (instance, dir)
    };

    if instance.loader == LoaderKind::Vanilla {
        return Err(LauncherError::msg(
            "Vanilla instances don't need a loader installed.",
        ));
    }

    let resolved = match loader_version {
        Some(version) => version,
        None => {
            let versions =
                loader::list_loader_versions(&state.http, instance.loader, &instance.game_version)
                    .await?;
            versions
                .iter()
                .find(|v| v.stable)
                .or_else(|| versions.first())
                .map(|v| v.version.clone())
                .ok_or_else(|| {
                    LauncherError::msg(format!(
                        "No {} build is available for Minecraft {}.",
                        instance.loader.as_str(),
                        instance.game_version
                    ))
                })?
        }
    };

    let java_path = match instance.loader {
        LoaderKind::Forge | LoaderKind::NeoForge => java::detect(&state.paths, None, 17)
            .await
            .map(|r| r.as_path())
            .ok_or_else(|| LauncherError::msg("Installing Forge requires Java 17 or newer."))?,
        _ => PathBuf::from("java"),
    };

    loader::install_loader(
        &state.http,
        &state.paths,
        &dir,
        instance.loader,
        &instance.game_version,
        &resolved,
        &java_path,
        state.sink(&app),
    )
    .await?;

    let mut updated = instance;
    updated.loader_version = Some(resolved);

    {
        let mut store = state.instances.write().await;
        store.get_mut(&id)?.loader_version = updated.loader_version.clone();
        store.persist(&state.paths, &id)?;
    }

    Ok(updated)
}

// ---------------------------------------------------------------------------
// Content management
// ---------------------------------------------------------------------------

/// One file inside an instance's content folders.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentEntry {
    pub name: String,
    pub path: String,
    pub enabled: bool,
    pub size: u64,
    /// `mod`, `resourcepack`, `shader`, `datapack`.
    pub kind: String,
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub title: Option<String>,
    pub icon_url: Option<String>,
}

fn content_kind(folder: &str) -> &'static str {
    match folder {
        "resourcepacks" => "resourcepack",
        "shaderpacks" => "shader",
        "datapacks" => "datapack",
        _ => "mod",
    }
}

/// Compare a tracked (possibly nested) path against a bare filename.
fn filename_matches(tracked: &str, file_name: &str) -> bool {
    tracked
        .rsplit('/')
        .next()
        .map(|name| name == file_name)
        .unwrap_or(false)
}

/// Accept only a listed content file directly inside one of the four
/// user-managed content folders. This keeps IPC callers from renaming or
/// deleting arbitrary files elsewhere in the game directory.
fn is_managed_content_file(root: &std::path::Path, path: &std::path::Path) -> bool {
    let (Ok(canonical_root), Ok(canonical_path)) = (root.canonicalize(), path.canonicalize()) else {
        return false;
    };
    let Some(parent) = canonical_path.parent() else {
        return false;
    };
    if !canonical_path.starts_with(&canonical_root) {
        return false;
    }
    let Some(name) = canonical_path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let base_name = name.strip_suffix(".disabled").unwrap_or(name);
    if !base_name.to_ascii_lowercase().ends_with(".jar")
        && !base_name.to_ascii_lowercase().ends_with(".zip")
    {
        return false;
    }

    ["mods", "resourcepacks", "shaderpacks", "datapacks"]
        .iter()
        .any(|folder| {
            let folder = canonical_root.join(folder);
            folder.canonicalize().ok().as_deref() == Some(parent)
                && canonical_path_is_within(&canonical_root, &folder)
        })
}

#[tauri::command]
pub async fn instance_content(state: State<'_, AppState>, id: String) -> Result<Vec<ContentEntry>> {
    let instance = state.instances.read().await.get(&id)?.clone();
    let dir = game_dir(&state.paths, &instance);

    let mut out = vec![];
    for folder in ["mods", "resourcepacks", "shaderpacks", "datapacks"] {
        let Ok(entries) = std::fs::read_dir(dir.join(folder)) else {
            continue;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().to_string();
            if file_name.starts_with('.') {
                continue;
            }

            // Accept `x.jar`, `x.zip` and their disabled variants.
            let base = file_name.trim_end_matches(".disabled");
            if !base.ends_with(".jar") && !base.ends_with(".zip") {
                continue;
            }

            // Attribution only from the project the file itself came from.
            // A pack-level record (legacy or offline installs) says the file
            // belongs to a pack but nothing about the mod, so those files
            // fall back to their filename.
            let tracked = instance.installed_projects.iter().find(|p| {
                p.project_type != "modpack"
                    && p.files
                        .iter()
                        .any(|f| filename_matches(f, &file_name) || filename_matches(f, base))
            });

            out.push(ContentEntry {
                name: file_name.clone(),
                path: entry.path().to_string_lossy().to_string(),
                enabled: !file_name.ends_with(".disabled"),
                size: entry.metadata().map(|m| m.len()).unwrap_or(0),
                kind: content_kind(folder).to_string(),
                project_id: tracked.map(|p| p.project_id.clone()),
                version_id: tracked.map(|p| p.version_id.clone()),
                title: tracked.map(|p| p.title.clone()),
                icon_url: tracked.and_then(|p| p.icon_url.clone()),
            });
        }
    }

    Ok(out)
}

/// Enable or disable a content file by toggling its `.disabled` suffix.
#[tauri::command]
pub async fn instance_toggle_content(
    state: State<'_, AppState>,
    id: String,
    path: String,
    enabled: bool,
) -> Result<String> {
    let instance = state.instances.read().await.get(&id)?.clone();
    let root = game_dir(&state.paths, &instance);
    let source = PathBuf::from(&path);
    if !source.is_file() || !is_managed_content_file(&root, &source) {
        return Err(LauncherError::msg("That file isn't part of this instance."));
    }
    let source = source.canonicalize()?;

    let target = if enabled {
        PathBuf::from(source.to_string_lossy().trim_end_matches(".disabled").to_string())
    } else {
        PathBuf::from(format!("{}.disabled", source.to_string_lossy()))
    };

    if !canonical_path_is_within(&root, target.parent().unwrap_or(&root)) {
        return Err(LauncherError::msg("That file isn't part of this instance."));
    }
    if target.exists() {
        return Err(LauncherError::msg("A file with that name already exists."));
    }
    std::fs::rename(&source, &target)?;
    Ok(target.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn instance_delete_content(
    state: State<'_, AppState>,
    id: String,
    path: String,
) -> Result<()> {
    let instance = state.instances.read().await.get(&id)?.clone();
    let root = game_dir(&state.paths, &instance);
    let target = PathBuf::from(&path);
    if !target.is_file() || !is_managed_content_file(&root, &target) {
        return Err(LauncherError::msg("That file isn't part of this instance."));
    }
    std::fs::remove_file(target)?;
    Ok(())
}

/// Import loose `.jar`/`.zip` files dropped onto the instance (drag & drop).
#[tauri::command]
pub async fn instance_import_files(
    state: State<'_, AppState>,
    id: String,
    paths: Vec<String>,
    folder: Option<String>,
) -> Result<usize> {
    let instance = state.instances.read().await.get(&id)?.clone();
    let folder = folder.unwrap_or_else(|| "mods".into());
    if !matches!(folder.as_str(), "mods" | "resourcepacks" | "shaderpacks" | "datapacks") {
        return Err(LauncherError::msg("Unsupported content folder."));
    }
    let game_root = game_dir(&state.paths, &instance);
    let dest_dir = game_root.join(folder);
    std::fs::create_dir_all(&dest_dir)?;
    if !canonical_path_is_within(&game_root, &dest_dir) {
        return Err(LauncherError::msg("The content folder resolves outside this instance."));
    }

    let mut copied = 0;
    for path in paths {
        let source = PathBuf::from(&path);
        if !source.is_file() {
            continue;
        }
        let name = source
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or_else(|| LauncherError::msg("invalid file name"))?;
        // Restrict filename to a single component.
        if !is_safe_filename(&name)
            || !name.to_ascii_lowercase().ends_with(".jar")
                && !name.to_ascii_lowercase().ends_with(".zip")
        {
            continue;
        }
        let dest = dest_dir.join(&name);
        if dest.exists() {
            continue;
        }
        std::fs::copy(&source, dest)?;
        copied += 1;
    }
    Ok(copied)
}

/// The directories the UI may reveal in the OS file manager.
#[tauri::command]
pub async fn instance_paths(
    state: State<'_, AppState>,
    id: String,
) -> Result<std::collections::HashMap<String, String>> {
    let instance = state.instances.read().await.get(&id)?.clone();
    let root = game_dir(&state.paths, &instance);

    let mut map = std::collections::HashMap::new();
    map.insert("game".into(), root.to_string_lossy().to_string());
    map.insert(
        "instance".into(),
        state.paths.instance_dir(&id).to_string_lossy().to_string(),
    );
    map.insert(
        "logs".into(),
        state
            .paths
            .instance_dir(&id)
            .join("logs")
            .to_string_lossy()
            .to_string(),
    );
    for folder in ["mods", "resourcepacks", "shaderpacks", "saves"] {
        map.insert(folder.into(), root.join(folder).to_string_lossy().to_string());
    }
    Ok(map)
}

/// Resolve the Modrinth-style content directory for a project type.
#[tauri::command]
pub fn instance_content_dir_name(project_type: String) -> String {
    match project_type.as_str() {
        "resourcepack" => "resourcepacks".into(),
        "shader" => "shaderpacks".into(),
        "datapack" => "datapacks".into(),
        _ => "mods".into(),
    }
}

/// Expose default settings so the new-instance dialog can prefill them.
#[tauri::command]
pub async fn instance_default_settings(state: State<'_, AppState>) -> Result<InstanceSettings> {
    let settings = state.settings.read().await;
    let mut defaults = InstanceSettings::default();
    defaults.memory_min = settings.default_memory_min;
    defaults.memory_max = settings.default_memory_max;
    Ok(defaults)
}

/// A generated, filesystem-friendly folder name for a new instance.
#[tauri::command]
pub fn instance_slug(name: String) -> String {
    slugify(&name)
}

// ---------------------------------------------------------------------------
// Modpacks
// ---------------------------------------------------------------------------

/// Read either pack format from disk: `.mrpack` first, then CurseForge
/// `manifest.json` zips.
fn read_pack_preview(pack: &std::path::Path) -> Result<modpack::ModpackPreview> {
    match modpack::read_index(pack) {
        Ok(index) => Ok(index.preview()),
        Err(_) => Ok(crate::core::cfpack::read_manifest(pack)?.preview()),
    }
}

#[tauri::command]
pub async fn modpack_preview(
    _state: State<'_, AppState>,
    path: String,
) -> Result<modpack::ModpackPreview> {
    read_pack_preview(&PathBuf::from(path))
}

/// Create a new instance from a local `.mrpack` or CurseForge pack `.zip`
/// and install it.
#[tauri::command]
pub async fn modpack_import(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    name: Option<String>,
) -> Result<Instance> {
    let pack = PathBuf::from(&path);

    // Not an .mrpack? Try a CurseForge pack zip before giving up.
    let index = match modpack::read_index(&pack) {
        Ok(index) => index,
        Err(_) => {
            return crate::commands::modrinth::install_cf_pack(app, state, pack, None, name).await;
        }
    };
    let (loader, loader_version) = index.loader_info();
    let game_version = index.game_version();

    let mut instance = Instance::new(
        name.unwrap_or_else(|| index.name.clone()),
        &game_version,
        loader,
    );
    instance.loader_version = loader_version.clone();
    instance.notes = index.summary.clone();

    let dir = game_dir(&state.paths, &instance);
    std::fs::create_dir_all(&dir)?;

    // Install the loader first so the pack lands in a working instance.
    if let Some(version) = loader_version.as_deref() {
        if loader != LoaderKind::Vanilla {
            let java_path = match loader {
                LoaderKind::Forge | LoaderKind::NeoForge => java::detect(&state.paths, None, 17)
                    .await
                    .map(|r| r.as_path())
                    .unwrap_or_else(|| PathBuf::from("java")),
                _ => PathBuf::from("java"),
            };
            loader::install_loader(
                &state.http,
                &state.paths,
                &dir,
                loader,
                &game_version,
                version,
                &java_path,
                state.sink(&app),
            )
            .await?;
        }
    }

    let concurrency = state.settings.read().await.concurrent_downloads;
    let mut projects = modpack::install(
        &state.http,
        &state.modrinth_base().await,
        &pack,
        &dir,
        concurrency,
        state.sink(&app),
    )
    .await?;

    // A file on disk carries no Modrinth ids, so the pack itself can only be
    // remembered by name — enough to say where the instance came from, and
    // skipped by update checks, which need a real id.
    projects.push(InstalledProject {
        project_id: String::new(),
        version_id: index.version_id.clone(),
        title: index.name.clone(),
        icon_url: None,
        project_type: "modpack".into(),
        files: vec![],
        installed_at: Some(chrono::Utc::now().to_rfc3339()),
    });
    instance.installed_projects = projects;

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

/// Export an instance to a `.mrpack`.
#[tauri::command]
pub async fn modpack_export(
    state: State<'_, AppState>,
    id: String,
    target: String,
) -> Result<String> {
    let instance = state.instances.read().await.get(&id)?.clone();
    let dir = game_dir(&state.paths, &instance);

    modpack::export(
        &PathBuf::from(&target),
        &instance.name,
        instance.notes.clone(),
        &instance.game_version,
        instance.loader,
        instance.loader_version.as_deref(),
        &dir,
        &instance.installed_projects,
    )?;
    Ok(target)
}
