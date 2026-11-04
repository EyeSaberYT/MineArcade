//! Modrinth and CurseForge browsing, installation and update commands.
//!
//! Everything routes by source: an explicit `source` argument picks the
//! catalog for searches, and `cf:`-prefixed ids always mean CurseForge —
//! which is how stored projects, pinned versions and dependency entries
//! come back to us later without anyone remembering where they came from.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, State};

use crate::api::curseforge::CurseForgeClient;
use crate::api::modrinth::{ModrinthClient, Project, ResolvedFile, SearchFilters, SearchResponse, Tag, Version};
use crate::core::download::{download_many, DownloadSpec, Progress, ProgressSink};
use crate::core::instance::{content_dir, game_dir, Instance, InstalledProject, LoaderKind};
use crate::core::path_security::{destination_is_within, is_safe_archive_path, is_safe_filename};
use crate::core::{cfpack, modpack};
use crate::core::{java, minecraft::loader};
use crate::error::{LauncherError, Result};
use crate::state::AppState;

async fn client(state: &AppState) -> ModrinthClient {
    ModrinthClient::new(state.http.clone(), state.modrinth_base().await)
}

fn cf_client(state: &AppState) -> CurseForgeClient {
    CurseForgeClient::new(state.http.clone())
}

/// Should this call go to CurseForge? `cf:` ids always do, whatever the
/// caller claimed; otherwise the explicit source wins.
fn use_curseforge(source: Option<&str>, id: &str) -> bool {
    id.starts_with("cf:") || source == Some("curseforge")
}

// ---------------------------------------------------------------------------
// Browsing
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn modrinth_search(
    state: State<'_, AppState>,
    filters: SearchFilters,
    source: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<SearchResponse> {
    if source.as_deref() == Some("curseforge") {
        return cf_client(&state)
            .search(&filters, limit.unwrap_or(20), offset.unwrap_or(0))
            .await;
    }
    client(&state)
        .await
        .search(&filters, limit.unwrap_or(20), offset.unwrap_or(0))
        .await
}

#[tauri::command]
pub async fn modrinth_project(state: State<'_, AppState>, id: String) -> Result<Project> {
    if use_curseforge(None, &id) {
        return cf_client(&state).project(&id).await;
    }
    client(&state).await.project(&id).await
}

#[tauri::command]
pub async fn modrinth_projects(
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> Result<Vec<Project>> {
    // Mixed lists can happen (a Modrinth pack tracking a CF dependency is
    // unlikely but legal), so split and merge rather than pick one source.
    let (cf_ids, mr_ids): (Vec<String>, Vec<String>) =
        ids.into_iter().partition(|id| id.starts_with("cf:"));

    let mut projects = if mr_ids.is_empty() {
        vec![]
    } else {
        client(&state).await.projects(&mr_ids).await?
    };
    if !cf_ids.is_empty() {
        let cf = cf_client(&state);
        for id in &cf_ids {
            // One broken id shouldn't blank the whole content list.
            if let Ok(project) = cf.project(id).await {
                projects.push(project);
            }
        }
    }
    Ok(projects)
}

#[tauri::command]
pub async fn modrinth_versions(
    state: State<'_, AppState>,
    project_id: String,
    game_version: Option<String>,
    loader: Option<String>,
) -> Result<Vec<Version>> {
    let game_versions = game_version.filter(|v| !v.is_empty()).into_iter().collect::<Vec<_>>();
    let loaders = loader.filter(|v| !v.is_empty()).into_iter().collect::<Vec<_>>();
    if use_curseforge(None, &project_id) {
        return cf_client(&state)
            .versions(&project_id, &game_versions, &loaders)
            .await;
    }
    client(&state)
        .await
        .versions(&project_id, &game_versions, &loaders)
        .await
}

/// Loader + version tags for the Discover filters.
#[tauri::command]
pub async fn modrinth_tags(state: State<'_, AppState>, kind: String) -> Result<Vec<Tag>> {
    client(&state).await.tags(&kind).await
}

// ---------------------------------------------------------------------------
// Installation
// ---------------------------------------------------------------------------

/// Which loader string to send to Modrinth for an instance.
fn loader_query(instance: &Instance, project_type: &str) -> Option<String> {
    // Resource packs and shaders aren't loader-specific.
    if matches!(project_type, "resourcepack" | "shader") {
        return None;
    }
    match instance.loader {
        LoaderKind::Vanilla => None,
        kind => Some(kind.modrinth_name().to_string()),
    }
}

/// Pick the best version for an instance: pinned, else newest compatible.
async fn resolve_version(
    client: &ModrinthClient,
    project_id: &str,
    project_type: &str,
    instance: &Instance,
    version_id: Option<String>,
) -> Result<Version> {
    if let Some(id) = version_id {
        return client.version(&id).await;
    }

    let game_versions = vec![instance.game_version.clone()];
    let loaders = loader_query(instance, project_type)
        .into_iter()
        .collect::<Vec<_>>();

    let mut versions = client
        .versions(project_id, &game_versions, &loaders)
        .await?;

    if versions.is_empty() && !loaders.is_empty() {
        // Some projects aren't tagged with the loader; retry without it.
        versions = client.versions(project_id, &game_versions, &[]).await?;
    }
    if versions.is_empty() {
        versions = client.versions(project_id, &[], &[]).await?;
    }

    versions
        .into_iter()
        .next()
        .ok_or_else(|| LauncherError::msg("No compatible version of that project was found."))
}

/// [`resolve_version`] for CurseForge projects (same fallback ladder, different
/// client — the two API clients share no trait).
async fn resolve_version_cf(
    client: &CurseForgeClient,
    project_id: &str,
    project_type: &str,
    instance: &Instance,
    version_id: Option<String>,
) -> Result<Version> {
    if let Some(id) = version_id {
        return client.version(&id).await;
    }

    let game_versions = vec![instance.game_version.clone()];
    let loaders = loader_query(instance, project_type)
        .into_iter()
        .collect::<Vec<_>>();

    let mut versions = client
        .versions(project_id, &game_versions, &loaders)
        .await?;

    if versions.is_empty() && !loaders.is_empty() {
        versions = client.versions(project_id, &game_versions, &[]).await?;
    }
    if versions.is_empty() {
        versions = client.versions(project_id, &[], &[]).await?;
    }

    versions
        .into_iter()
        .next()
        .ok_or_else(|| LauncherError::msg("No compatible version of that project was found."))
}

/// Download a set of resolved files into their correct content folders and
/// record them against the instance.
async fn install_resolved(
    state: &AppState,
    app: &AppHandle,
    instance_id: &str,
    resolved: Vec<ResolvedFile>,
    replace_project: bool,
) -> Result<Vec<InstalledProject>> {
    let (_, root) = {
        let store = state.instances.read().await;
        let instance = store.get(instance_id)?.clone();
        let root = game_dir(&state.paths, &instance);
        (instance, root)
    };

    let concurrency = state.settings.read().await.concurrent_downloads;
    let sink: ProgressSink = state.sink(app);

    let mut specs = vec![];
    let mut records: Vec<InstalledProject> = vec![];

    for file in &resolved {
        if !is_safe_filename(&file.filename) {
            return Err(LauncherError::msg("The provider returned an unsafe file name."));
        }
        let folder = content_dir(&root, &file.project_type);
        std::fs::create_dir_all(&folder)?;
        let dest = folder.join(&file.filename);
        if !destination_is_within(&root, &dest) {
            return Err(LauncherError::msg(
                "The content folder resolves outside this instance.",
            ));
        }

        let mut spec = DownloadSpec::new(file.url.clone(), &dest)
            .with_size(file.size)
            .with_label(file.title.clone());
        if let Some(sha1) = &file.sha1 {
            spec = spec.with_sha1(sha1.clone());
        }
        // CurseForge-hosted files need the API key on the CDN request.
        for (name, value) in crate::api::curseforge::download_headers(&file.url) {
            spec = spec.with_header(name, value);
        }
        specs.push(spec);

        records.push(InstalledProject {
            project_id: file.project_id.clone(),
            version_id: file.version_id.clone(),
            title: file.title.clone(),
            icon_url: file.icon_url.clone(),
            project_type: file.project_type.clone(),
            files: vec![file.filename.clone()],
            installed_at: Some(chrono::Utc::now().to_rfc3339()),
        });
    }

    sink(Progress::new(
        "install",
        format!(
            "Installing {} file{}…",
            specs.len(),
            if specs.len() == 1 { "" } else { "s" }
        ),
    ));

    download_many(state.http.clone(), specs, concurrency, "install", sink.clone()).await?;

    // Files just written. An update whose file name didn't change downloads
    // over the old file, which must then survive the cleanup below.
    let new_paths: Vec<PathBuf> = records
        .iter()
        .flat_map(|record| {
            record
                .files
                .iter()
                .filter_map(|file| tracked_file_path(&root, &record.project_type, file))
        })
        .collect();

    // Persist the new tracked content, removing stale entries for replaced projects.
    {
        let mut store = state.instances.write().await;
        let entry = store.get_mut(instance_id)?;
        for record in &records {
            // Remove old files belonging to this project when updating.
            if replace_project {
                if let Some(previous) = entry
                    .installed_projects
                    .iter()
                    .position(|p| p.project_id == record.project_id)
                {
                    let old = entry.installed_projects.remove(previous);
                    for file in old.files {
                        let Some(path) = tracked_file_path(&root, &old.project_type, &file) else {
                            continue;
                        };
                        if new_paths.contains(&path) {
                            continue;
                        }
                        if destination_is_within(&root, &path) && path.is_file() {
                            let _ = std::fs::remove_file(path);
                        }
                    }
                }
            }
            entry
                .installed_projects
                .retain(|p| !(p.project_id == record.project_id && p.version_id == record.version_id));
            entry.installed_projects.push(record.clone());
        }
        store.persist(&state.paths, instance_id)?;
    }

    sink(Progress {
        stage: "install".into(),
        label: "Installation complete".into(),
        current: 1,
        total: 1,
        files_done: 1,
        files_total: 1,
        finished: true,
    });

    Ok(records)
}

/// Where a tracked file lives. Single installs record a bare file name inside
/// the project type's folder; pack installs record a game-dir relative path
/// (`mods/a.jar`). `None` for anything unsafe.
fn tracked_file_path(root: &Path, project_type: &str, file: &str) -> Option<PathBuf> {
    if is_safe_filename(file) {
        Some(content_dir(root, project_type).join(file))
    } else if is_safe_archive_path(file) {
        Some(root.join(file.replace('/', std::path::MAIN_SEPARATOR_STR)))
    } else {
        None
    }
}

/// Install a project (and its required dependencies) into an instance.
#[tauri::command]
pub async fn modrinth_install(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    project_id: String,
    version_id: Option<String>,
) -> Result<Vec<InstalledProject>> {
    let instance = state.instances.read().await.get(&instance_id)?.clone();

    // CurseForge projects (detected by their `cf:` id) take the CF path.
    if project_id.starts_with("cf:") {
        let client = cf_client(&state);
        let project = client.project(&project_id).await?;

        if project.project_type == "modpack" {
            return Err(LauncherError::msg(
                "Modpacks create their own instance — use the pack installer instead.",
            ));
        }

        let version = resolve_version_cf(
            &client,
            &project_id,
            &project.project_type,
            &instance,
            version_id,
        )
        .await?;

        let mut resolved = vec![client.primary_file(&version).await?];

        let loader_name = instance.loader.modrinth_name().to_string();
        let dependencies = client
            .resolve_dependencies(&version, &loader_name, &instance.game_version, 4)
            .await
            .unwrap_or_default();

        let already_installed: Vec<String> = instance
            .installed_projects
            .iter()
            .map(|p| p.project_id.clone())
            .collect();
        for dependency in dependencies {
            if dependency.project_id != project_id && !already_installed.contains(&dependency.project_id) {
                resolved.push(dependency);
            }
        }

        return install_resolved(&state, &app, &instance_id, resolved, true).await;
    }

    let client = client(&state).await;
    let project = client.project(&project_id).await?;

    if project.project_type == "modpack" {
        return Err(LauncherError::msg(
            "Modpacks create their own instance — use the pack installer instead.",
        ));
    }

    let version = resolve_version(
        &client,
        &project_id,
        &project.project_type,
        &instance,
        version_id,
    )
    .await?;

    // Primary file first, then required dependencies (deduplicated).
    let mut resolved = vec![client.primary_file(&version).await?];

    let loader_name = instance.loader.modrinth_name().to_string();
    let dependencies = client
        .resolve_dependencies(&version, &loader_name, &instance.game_version, 4)
        .await
        .unwrap_or_default();

    let already_installed: Vec<String> = instance
        .installed_projects
        .iter()
        .map(|p| p.project_id.clone())
        .collect();
    for dependency in dependencies {
        if dependency.project_id != project_id && !already_installed.contains(&dependency.project_id) {
            resolved.push(dependency);
        }
    }

    install_resolved(&state, &app, &instance_id, resolved, true).await
}

/// Cache location for a downloaded pack, named after its version id. The
/// provider's file name is untrusted and not unique across packs; the id is
/// both once reduced to plain characters (`cf:1:2` → `cf_1_2`).
fn pack_cache_path(cache_dir: &Path, version_id: &str, extension: &str) -> PathBuf {
    let name: String = version_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .collect();
    cache_dir.join(format!("pack-{name}.{extension}"))
}

/// Install a Modrinth or CurseForge modpack as a brand new instance.
#[tauri::command]
pub async fn modrinth_install_modpack(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
    version_id: Option<String>,
    name: Option<String>,
) -> Result<Instance> {
    // CurseForge packs are plain zips with a manifest.json; the download and
    // install flow mirrors the .mrpack path below.
    if project_id.starts_with("cf:") {
        let client = cf_client(&state);
        let project = client.project(&project_id).await?;

        // Modpack versions aren't filtered by an existing instance's
        // loader/version, so take the pinned version or the newest one.
        let version = match version_id {
            Some(id) => client.version(&id).await?,
            None => client
                .versions(&project_id, &[], &[])
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| LauncherError::msg("That modpack has no published versions."))?,
        };
        let file = client.primary_file(&version).await?;
        let pack_path = pack_cache_path(&state.paths.cache_dir(), &version.id, "zip");

        let mut spec = DownloadSpec::new(file.url.clone(), &pack_path)
            .with_size(file.size)
            .with_label("modpack");
        if let Some(sha1) = file.sha1.clone().filter(|s| !s.is_empty()) {
            spec = spec.with_sha1(sha1);
        }
        for (header, value) in crate::api::curseforge::download_headers(&file.url) {
            spec = spec.with_header(header, value);
        }

        download_many(
            state.http.clone(),
            vec![spec],
            1,
            "modpack",
            state.sink(&app),
        )
        .await?;

        return install_cf_pack(app, state, pack_path, Some((&project, &version)), name).await;
    }

    let client = client(&state).await;
    let project = client.project(&project_id).await?;

    // Modpack versions aren't filtered by an existing instance's loader/version,
    // so just take the pinned version or the newest one.
    let version = match version_id {
        Some(id) => client.version(&id).await?,
        None => client
            .versions(&project_id, &[], &[])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| LauncherError::msg("That modpack has no published versions."))?,
    };
    let file = client.primary_file(&version).await?;

    // Download the .mrpack into the cache first.
    let pack_path = pack_cache_path(&state.paths.cache_dir(), &version.id, "mrpack");

    let mut spec = DownloadSpec::new(file.url.clone(), &pack_path)
        .with_size(file.size)
        .with_label("modpack");
    // Only verify when a checksum is actually published — an empty digest would
    // never match and would throw away the pack we just downloaded.
    if let Some(sha1) = file.sha1.clone().filter(|s| !s.is_empty()) {
        spec = spec.with_sha1(sha1);
    }

    download_many(
        state.http.clone(),
        vec![spec],
        1,
        "modpack",
        state.sink(&app),
    )
    .await?;

    let index = modpack::read_index(&pack_path)?;
    let (loader, loader_version) = index.loader_info();
    let game_version = index.game_version();

    let mut instance = Instance::new(
        name.unwrap_or_else(|| project.title.clone()),
        &game_version,
        loader,
    );
    instance.icon = project.icon_url.clone();
    instance.loader_version = loader_version.clone();
    instance.notes = Some(project.description.clone());

    let dir = game_dir(&state.paths, &instance);
    let new_dir = NewInstanceDir::create(&state, &instance)?;

    // Install the loader, then the pack contents.
    let installed = async {
        if let Some(loader_version) = loader_version.as_deref() {
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
                    loader_version,
                    &java_path,
                    state.sink(&app),
                )
                .await?;
            }
        }

        let concurrency = state.settings.read().await.concurrent_downloads;
        modpack::install(
            &state.http,
            &state.modrinth_base().await,
            &pack_path,
            &dir,
            concurrency,
            state.sink(&app),
        )
        .await
    }
    .await;
    let mut projects = new_dir.keep_if_ok(installed)?;

    // Remember the pack project itself so update checks can see it.
    projects.push(InstalledProject {
        project_id: project.id.clone(),
        version_id: version.id.clone(),
        title: project.title.clone(),
        icon_url: project.icon_url.clone(),
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

/// The folder of an instance being created. A failed install removes it
/// again, so no orphaned folder is left behind — unless it existed before.
pub(crate) struct NewInstanceDir {
    created: Option<PathBuf>,
}

impl NewInstanceDir {
    pub(crate) fn create(state: &AppState, instance: &Instance) -> Result<Self> {
        let root = state.paths.instance_dir(&instance.id);
        let existed = root.exists();
        std::fs::create_dir_all(game_dir(&state.paths, instance))?;
        Ok(Self {
            created: (!existed).then_some(root),
        })
    }

    /// Pass `result` through, removing the folder first when it's an error.
    pub(crate) fn keep_if_ok<T>(self, result: Result<T>) -> Result<T> {
        if result.is_err() {
            if let Some(root) = &self.created {
                let _ = std::fs::remove_dir_all(root);
            }
        }
        result
    }
}

/// Shared tail of every CurseForge pack install: read the manifest, create the
/// instance, install the loader, apply the pack, and persist.
///
/// `meta` carries the CurseForge project + version when the pack came from the
/// store; local `.zip` imports pass `None` and are tracked by name only.
pub(crate) async fn install_cf_pack(
    app: AppHandle,
    state: State<'_, AppState>,
    pack_path: PathBuf,
    meta: Option<(&Project, &Version)>,
    name: Option<String>,
) -> Result<Instance> {
    let manifest = cfpack::read_manifest(&pack_path)?;
    let (loader, loader_version) = manifest.loader_info();
    let game_version = manifest.minecraft.version.clone();

    let display_name = name
        .or_else(|| meta.map(|(p, _)| p.title.clone()))
        .unwrap_or_else(|| manifest.name.clone());
    let mut instance = Instance::new(display_name, &game_version, loader);
    instance.icon = meta.and_then(|(p, _)| p.icon_url.clone());
    instance.loader_version = loader_version.clone();
    instance.notes = meta.map(|(p, _)| p.description.clone());

    let dir = game_dir(&state.paths, &instance);
    let new_dir = NewInstanceDir::create(&state, &instance)?;

    // Install the loader first so the pack lands in a working instance.
    let installed = async {
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
        cfpack::install(
            &state.http,
            &manifest,
            &pack_path,
            &dir,
            concurrency,
            state.sink(&app),
        )
        .await
    }
    .await;
    let mut projects = new_dir.keep_if_ok(installed)?;

    // cfpack::install ends with a placeholder pack record; replace it with the
    // API-backed one when we know the pack's real ids, so update checks work.
    if let Some((project, version)) = meta {
        if let Some(record) = projects
            .iter_mut()
            .find(|p| p.project_type == "modpack" && p.project_id.is_empty())
        {
            record.project_id = project.id.clone();
            record.version_id = version.id.clone();
            record.title = project.title.clone();
            record.icon_url = project.icon_url.clone();
        }
    }
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

// ---------------------------------------------------------------------------
// Updates
// ---------------------------------------------------------------------------

/// An available update for one installed project.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub project_id: String,
    pub title: String,
    pub icon_url: Option<String>,
    pub current_version_id: String,
    pub latest_version_id: String,
    pub latest_version_name: String,
    pub changelog: Option<String>,
    pub project_type: String,
}

/// Check every tracked project in an instance for a newer compatible version.
#[tauri::command]
pub async fn modrinth_check_updates(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<Vec<UpdateInfo>> {
    check_updates(&state, &instance_id).await
}

/// Shared implementation so other commands can call it without going through
/// the Tauri command wrapper.
async fn check_updates(state: &AppState, instance_id: &str) -> Result<Vec<UpdateInfo>> {
    let instance = state.instances.read().await.get(instance_id)?.clone();
    let client = client(state).await;
    let cf = cf_client(state);

    let mut updates = vec![];
    for installed in &instance.installed_projects {
        // Local `.mrpack` imports and packs whose files couldn't be
        // attributed have no Modrinth id; there's nothing to ask about.
        if installed.project_id.is_empty() {
            continue;
        }
        // Orbit Client updates from its GitHub releases on launch.
        if installed.project_id == crate::commands::orbit::PROJECT_ID {
            continue;
        }
        // The pack record only says where the instance came from. Updating
        // means reinstalling a whole instance, which the per-file update
        // path can't do (it used to drop the pack archive into `mods/`).
        if installed.project_type == "modpack" {
            continue;
        }
        let is_cf = installed.project_id.starts_with("cf:");

        let game_versions = vec![instance.game_version.clone()];
        let loaders = loader_query(&instance, &installed.project_type)
            .into_iter()
            .collect::<Vec<_>>();

        let mut versions = if is_cf {
            match cf
                .versions(&installed.project_id, &game_versions, &loaders)
                .await
            {
                Ok(versions) => versions,
                Err(_) => continue,
            }
        } else {
            match client
                .versions(&installed.project_id, &game_versions, &loaders)
                .await
            {
                Ok(versions) => versions,
                Err(_) => continue,
            }
        };

        let Some(latest) = versions.drain(..).next() else {
            continue;
        };
        if latest.id == installed.version_id {
            continue;
        }

        let title = if is_cf {
            cf.project(&installed.project_id)
                .await
                .map(|p| p.title)
                .unwrap_or_else(|_| installed.title.clone())
        } else {
            client
                .project(&installed.project_id)
                .await
                .map(|p| p.title)
                .unwrap_or_else(|_| installed.title.clone())
        };

        updates.push(UpdateInfo {
            project_id: installed.project_id.clone(),
            title,
            icon_url: installed.icon_url.clone(),
            current_version_id: installed.version_id.clone(),
            latest_version_id: latest.id.clone(),
            latest_version_name: latest.version_number.clone(),
            changelog: latest.changelog.clone(),
            project_type: installed.project_type.clone(),
        });
    }

    Ok(updates)
}

/// Apply a single update by reinstalling the newest compatible version.
#[tauri::command]
pub async fn modrinth_apply_update(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    project_id: String,
    version_id: String,
) -> Result<()> {
    let _ = &project_id;
    let file = update_file(&state, &version_id).await?;
    install_resolved(&state, &app, &instance_id, vec![file], true).await?;
    Ok(())
}

/// The file an update installs. Refuses modpacks: their file is a pack
/// archive, not content, and must never land in a content folder.
async fn update_file(state: &AppState, version_id: &str) -> Result<ResolvedFile> {
    let file = if version_id.starts_with("cf:") {
        let cf = cf_client(state);
        let version = cf.version(version_id).await?;
        cf.primary_file(&version).await?
    } else {
        let client = client(state).await;
        let version = client.version(version_id).await?;
        client.primary_file(&version).await?
    };
    if file.project_type == "modpack" {
        return Err(LauncherError::msg(
            "Modpacks can't be updated in place — install the new version as a new instance.",
        ));
    }
    Ok(file)
}

/// Update every outdated project in an instance.
#[tauri::command]
pub async fn modrinth_update_all(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<usize> {
    let updates = check_updates(&state, &instance_id).await?;
    let count = updates.len();
    for update in updates {
        let file = update_file(&state, &update.latest_version_id).await?;
        install_resolved(&state, &app, &instance_id, vec![file], true).await?;
    }
    Ok(count)
}

/// Trigger the loader profile install if it's missing (used before launching).
pub async fn ensure_loader_installed(
    app: &AppHandle,
    state: &AppState,
    instance: &Instance,
) -> Result<()> {
    if instance.loader == LoaderKind::Vanilla {
        return Ok(());
    }
    let Some(loader_version) = instance.loader_version.clone() else {
        return Err(LauncherError::msg(
            "Choose a loader version for this instance first.",
        ));
    };
    let id = loader::profile_id(instance.loader, &instance.game_version, &loader_version);
    if loader::is_installed(&state.paths, &id) {
        return Ok(());
    }

    let dir = game_dir(&state.paths, instance);
    let java_path = match instance.loader {
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
        instance.loader,
        &instance.game_version,
        &loader_version,
        &java_path,
        state.sink(app),
    )
    .await?;
    Ok(())
}

/// Multithreaded dependency preview (used by the "install with dependencies" modal).
#[tauri::command]
pub async fn modrinth_dependencies(
    state: State<'_, AppState>,
    instance_id: String,
    version_id: String,
) -> Result<Vec<ResolvedFile>> {
    let instance = state.instances.read().await.get(&instance_id)?.clone();

    if version_id.starts_with("cf:") {
        let cf = cf_client(&state);
        let version = cf.version(&version_id).await?;
        return Ok(cf
            .resolve_dependencies(
                &version,
                instance.loader.modrinth_name(),
                &instance.game_version,
                4,
            )
            .await
            .unwrap_or_default());
    }

    let client = client(&state).await;
    let version = client.version(&version_id).await?;

    Ok(client
        .resolve_dependencies(
            &version,
            instance.loader.modrinth_name(),
            &instance.game_version,
            4,
        )
        .await
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_cache_names_come_from_the_version_id() {
        let cache = Path::new("cache");
        assert_eq!(pack_cache_path(cache, "AbC123", "mrpack"), cache.join("pack-AbC123.mrpack"));
        assert_eq!(pack_cache_path(cache, "cf:12:34", "zip"), cache.join("pack-cf_12_34.zip"));
        assert_eq!(pack_cache_path(cache, "../..\\x", "zip"), cache.join("pack-______x.zip"));
    }

    #[test]
    fn tracked_files_resolve_in_both_recorded_forms() {
        let root = Path::new("game");
        assert_eq!(
            tracked_file_path(root, "resourcepack", "a.zip"),
            Some(root.join("resourcepacks").join("a.zip"))
        );
        assert_eq!(
            tracked_file_path(root, "mod", "mods/a.jar"),
            Some(root.join("mods").join("a.jar"))
        );
        assert_eq!(tracked_file_path(root, "mod", "../a.jar"), None);
    }
}
