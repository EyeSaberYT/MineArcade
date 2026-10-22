//! Modrinth modpack (`.mrpack`) import and export.
//!
//! A `.mrpack` is a zip archive containing:
//! * `modrinth.index.json` – the manifest (files to download + dependencies)
//! * `overrides/` – files copied into every game directory
//! * `client-overrides/` – files copied only into client game directories
//!
//! See <https://docs.modrinth.com/docs/modpacks/format_definition/>.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::download::{download_many, DownloadSpec, Progress, ProgressSink};
use super::instance::{LoaderKind, InstalledProject};
use crate::api::modrinth::{ModrinthClient, Project, Version};
use crate::error::{LauncherError, Result};

pub const INDEX_FILE: &str = "modrinth.index.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MrpackIndex {
    pub format_version: u32,
    #[serde(default = "default_game")]
    pub game: String,
    #[serde(default)]
    pub version_id: String,
    pub name: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub files: Vec<MrpackFile>,
    #[serde(default)]
    pub dependencies: HashMap<String, String>,
}

fn default_game() -> String {
    "minecraft".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MrpackFile {
    /// Relative path inside the game directory, e.g. `mods/sodium.jar`.
    pub path: String,
    #[serde(default)]
    pub hashes: HashMap<String, String>,
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    #[serde(default)]
    pub downloads: Vec<String>,
    #[serde(default)]
    pub file_size: u64,
}

/// Details extracted from a pack before the user commits to installing it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModpackPreview {
    pub name: String,
    pub summary: Option<String>,
    pub version_id: String,
    pub game_version: String,
    pub loader: LoaderKind,
    pub loader_version: Option<String>,
    pub file_count: usize,
    pub total_size: u64,
    pub dependencies: HashMap<String, String>,
}

impl MrpackIndex {
    /// Derive the loader + version triple from the `dependencies` map.
    pub fn loader_info(&self) -> (LoaderKind, Option<String>) {
        for (key, kind) in [
            ("fabric-loader", LoaderKind::Fabric),
            ("quilt-loader", LoaderKind::Quilt),
            ("neoforge", LoaderKind::NeoForge),
            ("forge", LoaderKind::Forge),
        ] {
            if let Some(version) = self.dependencies.get(key) {
                return (kind, Some(version.clone()));
            }
        }
        (LoaderKind::Vanilla, None)
    }

    pub fn game_version(&self) -> String {
        self.dependencies
            .get("minecraft")
            .cloned()
            .unwrap_or_else(|| "1.20.1".into())
    }

    pub fn preview(&self) -> ModpackPreview {
        let (loader, loader_version) = self.loader_info();
        ModpackPreview {
            name: self.name.clone(),
            summary: self.summary.clone(),
            version_id: self.version_id.clone(),
            game_version: self.game_version(),
            loader,
            loader_version,
            file_count: self.files.len(),
            total_size: self.files.iter().map(|f| f.file_size).sum(),
            dependencies: self.dependencies.clone(),
        }
    }
}

/// Read `modrinth.index.json` from an `.mrpack` without extracting anything.
pub fn read_index(pack: &Path) -> Result<MrpackIndex> {
    // The path comes straight from a file dialog, so it may have been moved,
    // renamed or handed to us as something that isn't a file at all.
    let file = std::fs::File::open(pack).map_err(|e| {
        LauncherError::msg(format!(
            "Couldn't open '{}': {e}",
            pack.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| pack.display().to_string())
        ))
    })?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut entry = archive
        .by_name(INDEX_FILE)
        .map_err(|_| LauncherError::msg("This file isn't a valid Modrinth modpack (no index)."))?;
    let mut raw = String::new();
    entry.read_to_string(&mut raw)?;
    Ok(serde_json::from_str(&raw)?)
}

/// Install the pack into an existing (already created) instance game directory.
///
/// `modrinth_base` is only used to ask Modrinth which projects the pack's
/// files came from; the download itself works without it.
pub async fn install(
    http: &reqwest::Client,
    modrinth_base: &str,
    pack: &Path,
    game_dir: &Path,
    concurrency: usize,
    sink: ProgressSink,
) -> Result<Vec<InstalledProject>> {
    let index = read_index(pack)?;

    // ---- download declared files -----------------------------------------
    let mut specs = vec![];
    // (path, sha1) for every file that declares one, so the downloaded mods
    // can be attributed to their Modrinth project afterwards.
    let mut tracked: Vec<(String, String)> = vec![];
    for file in &index.files {
        // Skip server-only files when installing on the client.
        if let Some(env) = &file.env {
            if env.get("client").map(|v| v == "unsupported").unwrap_or(false) {
                continue;
            }
        }
        let Some(url) = file.downloads.first() else {
            continue;
        };
        // Basic filename guard: the pack must reference files inside `mods/`.
        // The path is taken from the mrpack index, not user input, but treat it
        // defensively and don't trust arbitrary paths.
        let path_str = &file.path;
        if path_str.contains("..") || path_str.contains('\\') {
            continue;
        }
        if path_str.starts_with('/') || path_str.starts_with('\\') {
            continue;
        }
        if let Some(c) = path_str.chars().next() {
            if c.is_ascii_alphabetic() && path_str.chars().nth(1) == Some(':') {
                continue;
            }
        }
        let dest = game_dir.join(path_str.replace('/', std::path::MAIN_SEPARATOR_STR));
        // Containment check: ensure dest stays inside game_dir.
        if let Ok(canon_dest) = dest.canonicalize() {
            if let Ok(canon_root) = game_dir.canonicalize() {
                if !canon_dest.starts_with(&canon_root) {
                    continue;
                }
            }
        } else {
            let mut rooted = true;
            let mut check = dest.as_path();
            while let Some(parent) = check.parent() {
                if parent.as_os_str().is_empty() {
                    rooted = false;
                    break;
                }
                if parent == game_dir {
                    break;
                }
                check = parent;
            }
            if !rooted {
                continue;
            }
        }
        let mut spec = DownloadSpec::new(url.clone(), dest).with_size(file.file_size);
        if let Some(sha1) = file.hashes.get("sha1") {
            spec = spec.with_sha1(sha1.clone());
            tracked.push((path_str.clone(), sha1.clone()));
        }
        specs.push(spec);
    }

    if !specs.is_empty() {
        download_many(http.clone(), specs, concurrency, "modpack", sink.clone()).await?;
    }

    // ---- extract overrides -----------------------------------------------
    sink(Progress::new("extract", "Applying modpack overrides…"));
    let pack = pack.to_path_buf();
    let dest = game_dir.to_path_buf();
    tokio::task::spawn_blocking(move || extract_overrides(&pack, &dest))
        .await
        .map_err(|e| LauncherError::msg(format!("override extraction failed: {e}")))??;

    // ---- attribution ------------------------------------------------------
    // Track each downloaded file as the Modrinth project it came from, so
    // update checks and the content list work per mod rather than per pack.
    let client = ModrinthClient::new(http.clone(), modrinth_base.to_string());
    Ok(resolve_projects(&client, &index, &tracked).await)
}

/// Ask Modrinth which projects a pack's downloaded files belong to.
///
/// A `.mrpack` carries no project or version ids at all — only URLs and
/// hashes — so hashing the files back to their source is the only way to
/// track them afterwards. Returns one record per project, or a single
/// record naming the pack itself when nothing could be attributed.
async fn resolve_projects(
    client: &ModrinthClient,
    index: &MrpackIndex,
    files: &[(String, String)],
) -> Vec<InstalledProject> {
    let hashes: Vec<String> = files
        .iter()
        .map(|(_, sha1)| sha1.to_ascii_lowercase())
        .collect();

    let Ok(found) = client.versions_from_hashes_batched(&hashes).await else {
        return vec![pack_record(index, files)];
    };
    // The API answers with its own key casing; normalise to match the index.
    let versions: HashMap<String, Version> = found
        .into_iter()
        .map(|(hash, version)| (hash.to_ascii_lowercase(), version))
        .collect();

    let mut ids: Vec<String> = versions
        .values()
        .map(|version| version.project_id.clone())
        .collect();
    ids.sort();
    ids.dedup();

    // Titles and icons are cosmetic, but the project type decides how update
    // checks filter versions, so `projects_batched` failing just degrades
    // those records to plain mods.
    let projects: HashMap<String, Project> = client
        .projects_batched(&ids)
        .await
        .into_iter()
        .map(|project| (project.id.clone(), project))
        .collect();

    let records = group_into_projects(files, &versions, &projects);
    if records.is_empty() {
        // Nothing matched — the pack ships files Modrinth has never seen.
        // Still remember them as belonging to the pack.
        return vec![pack_record(index, files)];
    }
    records
}

/// One tracking record per Modrinth project in `files`, in first-seen order.
///
/// Files that don't match any known hash are left out: they're custom content,
/// not something updates can act on.
fn group_into_projects(
    files: &[(String, String)],
    versions: &HashMap<String, Version>,
    projects: &HashMap<String, Project>,
) -> Vec<InstalledProject> {
    let mut records: Vec<InstalledProject> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();

    for (path, sha1) in files {
        let Some(version) = versions.get(&sha1.to_ascii_lowercase()) else {
            continue;
        };
        let project = projects.get(&version.project_id);

        let title = project.map(|p| p.title.clone()).unwrap_or_else(|| {
            if version.version_number.is_empty() {
                version.name.clone()
            } else {
                version.version_number.clone()
            }
        });

        let slot = *seen.entry(version.project_id.clone()).or_insert_with(|| {
            records.push(InstalledProject {
                project_id: version.project_id.clone(),
                version_id: version.id.clone(),
                title,
                icon_url: project.and_then(|p| p.icon_url.clone()),
                project_type: project
                    .map(|p| p.project_type.clone())
                    .unwrap_or_else(|| "mod".into()),
                files: vec![],
                installed_at: Some(chrono::Utc::now().to_rfc3339()),
            });
            records.len() - 1
        });
        records[slot].files.push(path.clone());
    }
    records
}

/// The record used when a pack's files couldn't be attributed to projects.
fn pack_record(index: &MrpackIndex, files: &[(String, String)]) -> InstalledProject {
    InstalledProject {
        // Empty on purpose. The mrpack format carries no Modrinth ids, and
        // inventing one out of the free-form `versionId` string (as early
        // builds did) made every lookup against it nonsense.
        project_id: String::new(),
        version_id: index.version_id.clone(),
        title: index.name.clone(),
        icon_url: None,
        project_type: "modpack".into(),
        files: files.iter().map(|(path, _)| path.clone()).collect(),
        installed_at: Some(chrono::Utc::now().to_rfc3339()),
    }
}

/// Copy `overrides/` and `client-overrides/` out of the archive.
fn extract_overrides(pack: &Path, game_dir: &Path) -> Result<()> {
    let file = std::fs::File::open(pack).map_err(|e| {
        LauncherError::msg(format!(
            "Couldn't open the pack archive '{}': {e}",
            pack.display()
        ))
    })?;
    let mut archive = zip::ZipArchive::new(file)?;

    for prefix in ["overrides/", "client-overrides/"] {
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let name = entry.name().to_string();
            let Some(relative) = name.strip_prefix(prefix) else {
                continue;
            };
            if relative.is_empty() || entry.is_dir() {
                continue;
            }
            if !super::path_security::is_safe_archive_path(relative) {
                continue;
            }
            let out_path = game_dir.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    LauncherError::msg(format!("Couldn't create {}: {e}", parent.display()))
                })?;
            }
            if !super::path_security::destination_is_within(game_dir, &out_path) {
                continue;
            }
            let mut out = std::fs::File::create(&out_path).map_err(|e| {
                LauncherError::msg(format!("Couldn't create {}: {e}", out_path.display()))
            })?;
            std::io::copy(&mut entry, &mut out).map_err(|e| {
                LauncherError::msg(format!("Couldn't write {}: {e}", out_path.display()))
            })?;
        }
    }
    Ok(())
}

/// Export an instance as a `.mrpack` at `target`.
///
/// Tracked Modrinth files go into the index (so they're re-downloadable) and
/// everything else in the game directory is bundled as an override.
pub fn export(
    target: &Path,
    name: &str,
    summary: Option<String>,
    game_version: &str,
    loader: LoaderKind,
    loader_version: Option<&str>,
    game_dir: &Path,
    projects: &[InstalledProject],
) -> Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(target)?;
    let mut writer = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // ---- index ------------------------------------------------------------
    let mut dependencies: HashMap<String, String> = HashMap::new();
    dependencies.insert("minecraft".into(), game_version.to_string());
    if let Some(version) = loader_version {
        let key = match loader {
            LoaderKind::Fabric => Some("fabric-loader"),
            LoaderKind::Quilt => Some("quilt-loader"),
            LoaderKind::Forge => Some("forge"),
            LoaderKind::NeoForge => Some("neoforge"),
            LoaderKind::Vanilla => None,
        };
        if let Some(key) = key {
            dependencies.insert(key.into(), version.to_string());
        }
    }

    let index = MrpackIndex {
        format_version: 1,
        game: "minecraft".into(),
        version_id: "1.0.0".into(),
        name: name.to_string(),
        summary,
        files: vec![],
        dependencies,
    };

    writer.start_file(INDEX_FILE, options)?;
    writer.write_all(serde_json::to_string_pretty(&index)?.as_bytes())?;

    // ---- overrides --------------------------------------------------------
    // Anything in the game dir that isn't tracked launcher metadata becomes an
    // override. Modrinth-installed content is represented in the index instead.
    let tracked: std::collections::HashSet<String> = projects
        .iter()
        .flat_map(|p| p.files.iter().cloned())
        .collect();

    let skip_dirs = ["versions", "logs", "crash-reports", "screenshots", "runtime", "natives"];
    for entry in walkdir::WalkDir::new(game_dir)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !skip_dirs.contains(&name.as_ref())
        })
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = match entry.path().strip_prefix(game_dir) {
            Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        if tracked.contains(&relative) {
            continue;
        }

        writer.start_file(format!("overrides/{relative}"), options)?;
        let mut source = std::fs::File::open(entry.path())?;
        let mut buffer = vec![];
        source.read_to_end(&mut buffer)?;
        writer.write_all(&buffer)?;
    }

    writer.finish()?;
    Ok(())
}

/// Best-effort guess of the loader from a Modrinth pack dependency map.
pub fn loader_from_dependencies(deps: &HashMap<String, String>) -> (LoaderKind, Option<String>) {
    let index = MrpackIndex {
        format_version: 1,
        game: "minecraft".into(),
        version_id: String::new(),
        name: String::new(),
        summary: None,
        files: vec![],
        dependencies: deps.clone(),
    };
    index.loader_info()
}

/// Convenience: reveal the files a pack will place in `mods/`.
pub fn mod_files(pack: &Path) -> Result<Vec<String>> {
    let index = read_index(pack)?;
    Ok(index
        .files
        .into_iter()
        .map(|f| f.path)
        .filter(|p| p.starts_with("mods/"))
        .collect())
}

/// Resolve a destination path safely, refusing to escape the game directory.
pub fn safe_join(root: &Path, relative: &str) -> Option<PathBuf> {
    let candidate = root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    let normalized = candidate.canonicalize().ok()?;
    let root = root.canonicalize().ok()?;
    normalized.starts_with(root).then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(id: &str, project_id: &str, number: &str) -> Version {
        Version {
            id: id.into(),
            project_id: project_id.into(),
            name: id.into(),
            version_number: number.into(),
            ..Version::default()
        }
    }

    fn project(id: &str, title: &str, project_type: &str) -> Project {
        Project {
            id: id.into(),
            title: title.into(),
            project_type: project_type.into(),
            ..Project::default()
        }
    }

    #[test]
    fn groups_files_by_project() {
        let versions = HashMap::from([
            ("aaa".to_string(), version("v1", "p1", "1.0")),
            ("bbb".to_string(), version("v2", "p1", "1.1")),
            ("ccc".to_string(), version("v3", "p2", "2.0")),
        ]);
        let projects = HashMap::from([
            ("p1".to_string(), project("p1", "Alpha", "mod")),
            ("p2".to_string(), project("p2", "Beta", "resourcepack")),
        ]);
        // Hash casing differs between the index (lower) and the API (upper).
        let files = vec![
            ("mods/a.jar".to_string(), "AAA".to_string()),
            ("mods/b.jar".to_string(), "bbb".to_string()),
            ("resourcepacks/c.zip".to_string(), "ccc".to_string()),
        ];

        let records = group_into_projects(&files, &versions, &projects);

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].project_id, "p1");
        assert_eq!(records[0].title, "Alpha");
        assert_eq!(records[0].version_id, "v1"); // first file's version wins
        assert_eq!(records[0].files, vec!["mods/a.jar", "mods/b.jar"]);
        assert_eq!(records[1].project_type, "resourcepack");
        assert_eq!(records[1].files, vec!["resourcepacks/c.zip"]);
    }

    #[test]
    fn unknown_files_are_left_untracked() {
        let versions = HashMap::from([("aaa".to_string(), version("v1", "p1", "1.0"))]);
        let files = vec![
            ("mods/a.jar".to_string(), "aaa".to_string()),
            ("mods/custom.jar".to_string(), "fff".to_string()),
        ];

        let records = group_into_projects(&files, &versions, &HashMap::new());

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].files, vec!["mods/a.jar"]);
    }

    #[test]
    fn missing_project_metadata_degrades_gracefully() {
        let versions = HashMap::from([("aaa".to_string(), version("v1", "p1", "1.0"))]);
        let files = vec![("mods/a.jar".to_string(), "aaa".to_string())];

        let records = group_into_projects(&files, &versions, &HashMap::new());

        assert_eq!(records[0].title, "1.0"); // from the version, not the project
        assert_eq!(records[0].project_type, "mod");
        assert_eq!(records[0].icon_url, None);
    }

    #[test]
    fn pack_record_never_invents_an_id() {
        let index = MrpackIndex {
            format_version: 1,
            game: "minecraft".into(),
            version_id: "26.1.2-1.1".into(),
            name: "Vulkan Optimized".into(),
            summary: None,
            files: vec![],
            dependencies: HashMap::new(),
        };
        let files = vec![("mods/a.jar".to_string(), "aaa".to_string())];

        let record = pack_record(&index, &files);

        assert!(record.project_id.is_empty());
        assert_eq!(record.version_id, "26.1.2-1.1");
        assert_eq!(record.title, "Vulkan Optimized");
        assert_eq!(record.project_type, "modpack");
        assert_eq!(record.files, vec!["mods/a.jar"]);
    }
}
