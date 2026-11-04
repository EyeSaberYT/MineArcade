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
        specs.push(download_spec(file, game_dir)?);
        if let Some(sha1) = file.hashes.get("sha1").filter(|h| !h.is_empty()) {
            tracked.push((file.path.clone(), sha1.clone()));
        }
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

/// Hosts a `.mrpack` may download from, per Modrinth's format definition.
const ALLOWED_DOWNLOAD_HOSTS: [&str; 4] = [
    "cdn.modrinth.com",
    "github.com",
    "raw.githubusercontent.com",
    "gitlab.com",
];

/// True for an `https` URL on one of [`ALLOWED_DOWNLOAD_HOSTS`].
fn allowed_download_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|parsed| {
        parsed.scheme() == "https"
            && parsed
                .host_str()
                .is_some_and(|host| ALLOWED_DOWNLOAD_HOSTS.contains(&host))
    })
}

/// Validate one index entry and turn it into a download.
///
/// The index is untrusted input: a file without a checksum, outside the game
/// directory, or hosted anywhere but Modrinth's allowlist fails the install
/// rather than being fetched (or silently skipped).
fn download_spec(file: &MrpackFile, game_dir: &Path) -> Result<DownloadSpec> {
    let path = &file.path;
    let hash = |algo: &str| file.hashes.get(algo).filter(|h| !h.is_empty()).cloned();
    let (sha1, sha512) = (hash("sha1"), hash("sha512"));
    if sha1.is_none() && sha512.is_none() {
        return Err(LauncherError::msg(format!(
            "The modpack lists '{path}' without a checksum, so it can't be verified."
        )));
    }

    if !super::path_security::is_safe_archive_path(path) {
        return Err(LauncherError::msg(format!(
            "The modpack tries to write to an unsafe path: '{path}'."
        )));
    }
    let dest = game_dir.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            LauncherError::msg(format!("Couldn't create {}: {e}", parent.display()))
        })?;
    }
    if !super::path_security::destination_is_within(game_dir, &dest) {
        return Err(LauncherError::msg(format!(
            "The modpack file '{path}' resolves outside the instance."
        )));
    }

    let mut urls = file.downloads.iter().filter(|url| allowed_download_url(url)).cloned();
    let Some(url) = urls.next() else {
        return Err(LauncherError::msg(format!(
            "The modpack file '{path}' has no download from an allowed host ({}).",
            ALLOWED_DOWNLOAD_HOSTS.join(", ")
        )));
    };

    let mut spec = DownloadSpec::new(url, dest)
        .with_size(file.file_size)
        .with_fallback_urls(urls.collect());
    if let Some(sha1) = sha1 {
        spec = spec.with_sha1(sha1);
    }
    if let Some(sha512) = sha512 {
        spec = spec.with_sha512(sha512);
    }
    Ok(spec)
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

    // ---- files and overrides ----------------------------------------------
    // Content installed from Modrinth goes into the index so it's
    // re-downloaded from its source; everything else in the game dir
    // (CurseForge files, hand-added mods, configs) becomes an override.
    let downloads = modrinth_downloads(projects);
    let mut files: Vec<MrpackFile> = vec![];

    for entry in walkdir::WalkDir::new(game_dir)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !EXPORT_SKIP_DIRS.contains(&name.as_ref())
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

        let mut source = std::fs::File::open(entry.path())?;
        let mut buffer = vec![];
        source.read_to_end(&mut buffer)?;

        if let Some(url) = downloads.get(&relative) {
            files.push(index_file(relative, url.clone(), &buffer));
            continue;
        }

        writer.start_file(format!("overrides/{relative}"), options)?;
        writer.write_all(&buffer)?;
    }

    let index = MrpackIndex {
        format_version: 1,
        game: "minecraft".into(),
        version_id: "1.0.0".into(),
        name: name.to_string(),
        summary,
        files,
        dependencies,
    };

    writer.start_file(INDEX_FILE, options)?;
    writer.write_all(serde_json::to_string_pretty(&index)?.as_bytes())?;

    writer.finish()?;
    Ok(())
}

/// Top-level game-dir folders that never belong in an exported pack: launcher
/// and game output, plus world backups (`<game dir>/backups`).
const EXPORT_SKIP_DIRS: [&str; 7] = [
    "versions",
    "logs",
    "crash-reports",
    "screenshots",
    "runtime",
    "natives",
    "backups",
];

/// Game-dir relative path → Modrinth CDN URL for every tracked file that was
/// installed from Modrinth and can be re-downloaded from it.
///
/// Files tracked by other sources (CurseForge, Orbit Client, the pack record
/// of a local import) aren't on Modrinth's CDN and stay out of the map, so
/// they're exported as overrides. A record holding several files only knows
/// one version id, so its files can't all be addressed and stay overrides too.
fn modrinth_downloads(projects: &[InstalledProject]) -> HashMap<String, String> {
    let is_modrinth_id = |id: &str| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric());

    let mut out = HashMap::new();
    for project in projects {
        if project.project_type == "modpack"
            || !is_modrinth_id(&project.project_id)
            || !is_modrinth_id(&project.version_id)
        {
            continue;
        }
        let [file] = project.files.as_slice() else {
            continue;
        };
        // Pack installs record game-dir paths (`mods/a.jar`); single installs
        // record bare file names inside the project type's folder.
        let relative = if file.contains('/') {
            if !super::path_security::is_safe_archive_path(file) {
                continue;
            }
            file.clone()
        } else {
            if !super::path_security::is_safe_filename(file) {
                continue;
            }
            let folder = super::instance::content_dir(Path::new(""), &project.project_type);
            format!("{}/{file}", folder.to_string_lossy())
        };
        let Some(filename) = relative.rsplit('/').next() else {
            continue;
        };
        let url = format!(
            "https://cdn.modrinth.com/data/{}/versions/{}/{}",
            project.project_id,
            project.version_id,
            crate::api::modrinth::urlencode(filename)
        );
        out.insert(relative, url);
    }
    out
}

/// An index entry for a file being exported, hashed from its contents.
fn index_file(path: String, url: String, contents: &[u8]) -> MrpackFile {
    use sha1::{Digest, Sha1};
    use sha2::Sha512;

    let hashes = HashMap::from([
        ("sha1".to_string(), super::download::to_hex(&Sha1::digest(contents))),
        ("sha512".to_string(), super::download::to_hex(&Sha512::digest(contents))),
    ]);
    MrpackFile {
        path,
        hashes,
        env: None,
        downloads: vec![url],
        file_size: contents.len() as u64,
    }
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

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbit-mrpack-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mrpack_file(path: &str, hashes: &[(&str, &str)], downloads: &[&str]) -> MrpackFile {
        MrpackFile {
            path: path.into(),
            hashes: hashes.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            env: None,
            downloads: downloads.iter().map(|u| u.to_string()).collect(),
            file_size: 1,
        }
    }

    #[test]
    fn download_spec_validates_untrusted_index_entries() {
        let game = temp_dir("spec");
        let cdn = "https://cdn.modrinth.com/data/x/versions/y/a.jar";

        // No checksum, unsafe paths and foreign or plain-http hosts all fail.
        assert!(download_spec(&mrpack_file("mods/a.jar", &[], &[cdn]), &game).is_err());
        assert!(download_spec(&mrpack_file("mods/a.jar", &[("sha1", "")], &[cdn]), &game).is_err());
        for path in ["../a.jar", "mods/../../a.jar", "C:/a.jar", "mods/a.jar:ads", ""] {
            assert!(download_spec(&mrpack_file(path, &[("sha1", "aa")], &[cdn]), &game).is_err(), "{path}");
        }
        for url in ["https://evil.example/a.jar", "http://cdn.modrinth.com/a.jar"] {
            assert!(download_spec(&mrpack_file("mods/a.jar", &[("sha1", "aa")], &[url]), &game).is_err(), "{url}");
        }

        // Allowed URLs are kept in order; disallowed ones are dropped.
        let spec = download_spec(
            &mrpack_file(
                "mods/foo..bar.jar",
                &[("sha512", "bb")],
                &["https://evil.example/a.jar", cdn, "https://github.com/o/r/a.jar"],
            ),
            &game,
        )
        .unwrap();
        assert_eq!(spec.url, cdn);
        assert_eq!(spec.fallback_urls, vec!["https://github.com/o/r/a.jar"]);
        assert_eq!(spec.sha512.as_deref(), Some("bb"));
        assert_eq!(spec.sha1, None);
        assert!(spec.dest.ends_with("foo..bar.jar"));
        let _ = std::fs::remove_dir_all(&game);
    }

    fn tracked(project_id: &str, version_id: &str, project_type: &str, files: &[&str]) -> InstalledProject {
        InstalledProject {
            project_id: project_id.into(),
            version_id: version_id.into(),
            title: project_id.into(),
            icon_url: None,
            project_type: project_type.into(),
            files: files.iter().map(|f| f.to_string()).collect(),
            installed_at: None,
        }
    }

    #[test]
    fn only_modrinth_sourced_files_get_download_urls() {
        let downloads = modrinth_downloads(&[
            tracked("AANobbMI", "NM9w06Kr", "mod", &["sodium 1.jar"]),
            tracked("P7dR8mSH", "abc12345", "resourcepack", &["resourcepacks/pack.zip"]),
            tracked("cf:123", "cf:123:456", "mod", &["cf.jar"]),
            tracked("orbit-client", "v1.0.0", "mod", &["orbit-client-1.jar"]),
            tracked("u76hn1pF", "aaaaaaaa", "modpack", &["mods/packed.jar"]),
            tracked("Abcdefgh", "Bcdefghi", "mod", &["two.jar", "three.jar"]),
        ]);

        assert_eq!(downloads.len(), 2, "{downloads:?}");
        assert_eq!(
            downloads["mods/sodium 1.jar"],
            "https://cdn.modrinth.com/data/AANobbMI/versions/NM9w06Kr/sodium%201.jar"
        );
        assert_eq!(
            downloads["resourcepacks/pack.zip"],
            "https://cdn.modrinth.com/data/P7dR8mSH/versions/abc12345/pack.zip"
        );
    }

    #[test]
    fn export_indexes_modrinth_files_and_skips_backups() {
        let root = temp_dir("export");
        let game = root.join("game");
        for (path, contents) in [
            ("mods/sodium.jar", "sodium"),
            ("mods/cf.jar", "cf"),
            ("config/a.toml", "x"),
            ("backups/world.zip", "backup"),
            ("logs/latest.log", "log"),
        ] {
            let file = game.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, contents).unwrap();
        }
        let target = root.join("out.mrpack");
        export(
            &target,
            "Pack",
            None,
            "1.21.1",
            LoaderKind::Fabric,
            Some("0.16.9"),
            &game,
            &[
                tracked("AANobbMI", "NM9w06Kr", "mod", &["sodium.jar"]),
                tracked("cf:1", "cf:1:2", "mod", &["cf.jar"]),
            ],
        )
        .unwrap();

        let index = read_index(&target).unwrap();
        assert_eq!(index.files.len(), 1);
        let file = &index.files[0];
        assert_eq!(file.path, "mods/sodium.jar");
        assert_eq!(file.file_size, 6);
        assert_eq!(file.hashes["sha1"].len(), 40);
        assert_eq!(file.hashes["sha512"].len(), 128);
        assert_eq!(
            file.downloads,
            vec!["https://cdn.modrinth.com/data/AANobbMI/versions/NM9w06Kr/sodium.jar"]
        );

        let archive = zip::ZipArchive::new(std::fs::File::open(&target).unwrap()).unwrap();
        let mut names: Vec<&str> = archive.file_names().collect();
        names.sort();
        assert_eq!(
            names,
            vec![INDEX_FILE, "overrides/config/a.toml", "overrides/mods/cf.jar"]
        );
        let _ = std::fs::remove_dir_all(&root);
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
