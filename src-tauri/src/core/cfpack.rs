//! CurseForge modpack (`.zip` with a `manifest.json`) support.
//!
//! CurseForge packs ship as a plain zip archive containing:
//! * `manifest.json` – Minecraft version, loader, and the CurseForge file ids
//!   of every mod in the pack
//! * `overrides/` (or a custom folder named by the manifest) – files copied
//!   verbatim into the game directory
//!
//! Unlike `.mrpack`, the format carries no URLs: each entry is a
//! `(projectID, fileID)` pair that has to be resolved through the API.
//!
//! See <https://support.curseforge.com/en/support/solutions/articles/29000059189>.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use serde::Deserialize;

use super::download::{download_many, DownloadSpec, Progress, ProgressSink};
use super::instance::{InstalledProject, LoaderKind};
use super::modpack::ModpackPreview;
use super::path_security::{
    destination_is_within, is_safe_archive_path, is_safe_filename,
};
use crate::api::curseforge::{self, CurseForgeClient};
use crate::api::modrinth::{Project, Version};
use crate::error::{LauncherError, Result};

/// Ids per batch resolution request, comfortably under the API's limits.
const BATCH: usize = 50;

/// The `manifest.json` at the root of a CurseForge pack.
#[derive(Debug, Clone, Deserialize)]
pub struct CfManifest {
    pub minecraft: CfManifestMinecraft,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub files: Vec<CfManifestFile>,
    /// Folder inside the archive whose contents land in the game directory.
    #[serde(rename = "overrides", default = "default_overrides")]
    pub overrides: String,
}

fn default_overrides() -> String {
    "overrides".into()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CfManifestMinecraft {
    pub version: String,
    #[serde(default)]
    pub mod_loaders: Vec<CfModLoader>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CfModLoader {
    /// e.g. `fabric-0.16.9`, `forge-47.2.0`, `neoforge-21.1.172`.
    pub id: String,
    #[serde(default)]
    pub primary: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CfManifestFile {
    #[serde(rename = "projectID")]
    pub project_id: i64,
    #[serde(rename = "fileID")]
    pub file_id: i64,
    #[serde(default = "default_true")]
    pub required: bool,
}

fn default_true() -> bool {
    true
}

impl CfManifest {
    /// Derive the loader + version triple from the `modLoaders` list.
    ///
    /// The primary entry wins; otherwise the first one. Entries that don't
    /// name a loader we can install (`minecraft`, legacy `rift`, …) fall back
    /// to vanilla rather than failing the whole pack.
    pub fn loader_info(&self) -> (LoaderKind, Option<String>) {
        let chosen = self
            .minecraft
            .mod_loaders
            .iter()
            .find(|l| l.primary)
            .or_else(|| self.minecraft.mod_loaders.first());

        let Some(loader) = chosen else {
            return (LoaderKind::Vanilla, None);
        };
        let (name, version) = loader.id.split_once('-').unwrap_or((&loader.id, ""));
        let kind = match name.to_ascii_lowercase().as_str() {
            "fabric" => LoaderKind::Fabric,
            "quilt" => LoaderKind::Quilt,
            "neoforge" => LoaderKind::NeoForge,
            "forge" => LoaderKind::Forge,
            // Vanilla has no loader version to speak of — a `minecraft-1.21.1`
            // style suffix would only pollute the instance record.
            _ => return (LoaderKind::Vanilla, None),
        };
        let version = (!version.is_empty()).then(|| version.to_string());
        (kind, version)
    }

    pub fn preview(&self) -> ModpackPreview {
        let (loader, loader_version) = self.loader_info();
        ModpackPreview {
            name: self.name.clone(),
            summary: None,
            version_id: self.version.clone(),
            game_version: self.minecraft.version.clone(),
            loader,
            loader_version,
            file_count: self.files.len(),
            // Sizes aren't in the manifest; the caller can leave this at 0.
            total_size: 0,
            dependencies: Default::default(),
        }
    }
}

/// Read `manifest.json` from a CurseForge pack zip without extracting anything.
pub fn read_manifest(pack: &Path) -> Result<CfManifest> {
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
    let mut entry = archive.by_name("manifest.json").map_err(|_| {
        LauncherError::msg("This file isn't a CurseForge modpack (no manifest.json).")
    })?;
    let mut raw = String::new();
    entry.read_to_string(&mut raw)?;
    Ok(serde_json::from_str(&raw)?)
}

/// Copy the pack's override folder into the game directory.
///
/// `overrides_dir` is the manifest's `overrides` value (usually just
/// "overrides"). Entries that try to escape the archive root are skipped.
pub fn extract_overrides(pack: &Path, game_dir: &Path, overrides_dir: &str) -> Result<()> {
    let file = std::fs::File::open(pack).map_err(|e| {
        LauncherError::msg(format!(
            "Couldn't open the pack archive '{}': {e}",
            pack.display()
        ))
    })?;
    let mut archive = zip::ZipArchive::new(file)?;
    let prefix = format!("{}/", overrides_dir.trim_end_matches('/'));

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_string();
        let Some(relative) = name.strip_prefix(&prefix) else {
            continue;
        };
        if relative.is_empty() || entry.is_dir() {
            continue;
        }
        if !is_safe_archive_path(relative) {
            continue;
        }
        let out_path = game_dir.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                LauncherError::msg(format!("Couldn't create {}: {e}", parent.display()))
            })?;
        }
        if !destination_is_within(game_dir, &out_path) {
            continue;
        }
        let mut out = std::fs::File::create(&out_path).map_err(|e| {
            LauncherError::msg(format!("Couldn't create {}: {e}", out_path.display()))
        })?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|e| LauncherError::msg(format!("Couldn't write {}: {e}", out_path.display())))?;
    }
    Ok(())
}

/// Install a CurseForge pack into an existing (already created) instance game
/// directory: resolve every manifest entry through the API, download the mods,
/// then apply the overrides folder.
///
/// Returns one tracking record per mod, plus a record for the pack itself.
pub async fn install(
    http: &reqwest::Client,
    manifest: &CfManifest,
    pack: &Path,
    game_dir: &Path,
    concurrency: usize,
    sink: ProgressSink,
) -> Result<Vec<InstalledProject>> {
    let client = CurseForgeClient::new(http.clone());

    // ---- resolve every manifest file id -----------------------------------
    sink(Progress::new("modpack", "Resolving pack files…"));
    let file_ids: Vec<i64> = manifest.files.iter().map(|f| f.file_id).collect();
    let mut versions: Vec<Version> = vec![];
    let mut ok_chunks = 0usize;
    for chunk in file_ids.chunks(BATCH) {
        match client.files_batch(chunk).await {
            Ok(batch) => {
                versions.extend(batch);
                ok_chunks += 1;
            }
            // A single dead file id shouldn't sink a 300-mod pack; only fail
            // outright when nothing resolved at all.
            Err(_) => continue,
        }
    }
    if ok_chunks == 0 && !file_ids.is_empty() {
        return Err(LauncherError::msg(
            "CurseForge didn't return any of the pack's files — check your connection and try again.",
        ));
    }

    // Files published without an inline URL fall back to the download-url
    // endpoint, one lookup at a time (rare enough not to batch).
    for version in &mut versions {
        if version
            .files
            .first()
            .map(|f| f.url.is_empty())
            .unwrap_or(true)
        {
            let Some(mod_id) = curseforge::modrinth_id_to_cf(&version.project_id) else {
                continue;
            };
            let file_id = version.id.rsplit(':').next().and_then(|f| f.parse().ok());
            let Some(file_id) = file_id else { continue };
            if let Ok(url) = client.download_url(mod_id, file_id).await {
                if let Some(file) = version.files.first_mut() {
                    file.url = url;
                }
            }
        }
    }

    // ---- titles and icons (cosmetic; failure degrades to file names) ------
    let mut mod_ids: Vec<i64> = versions
        .iter()
        .filter_map(|v| curseforge::modrinth_id_to_cf(&v.project_id))
        .collect();
    mod_ids.sort_unstable();
    mod_ids.dedup();
    let mut projects: HashMap<String, Project> = HashMap::new();
    for chunk in mod_ids.chunks(BATCH) {
        if let Ok(batch) = client.mods_batch(chunk).await {
            for project in batch {
                projects.insert(project.id.clone(), project);
            }
        }
    }

    // ---- download the mods ------------------------------------------------
    let mut specs = vec![];
    let mut records: Vec<InstalledProject> = vec![];
    let mods_dir = game_dir.join("mods");
    std::fs::create_dir_all(&mods_dir)?;
    if !destination_is_within(game_dir, &mods_dir) {
        return Err(LauncherError::msg(
            "The mods folder resolves outside this instance.",
        ));
    }

    for version in &versions {
        let Some(file) = version.files.first() else {
            continue;
        };
        if file.url.is_empty() {
            continue;
        }
        if !is_safe_filename(&file.filename) {
            log::warn!("Skipping CurseForge file with unsafe filename: {:?}", file.filename);
            continue;
        }
        let filename = file.filename.clone();
        let mut spec = DownloadSpec::new(&file.url, mods_dir.join(&filename))
            .with_size(file.size)
            .with_label(&file.filename);
        if let Some(sha1) = file.hashes.get("sha1") {
            spec = spec.with_sha1(sha1.clone());
        }
        for (name, value) in curseforge::download_headers(&file.url) {
            spec = spec.with_header(name, value);
        }
        specs.push(spec);

        let project = projects.get(&version.project_id);
        records.push(InstalledProject {
            project_id: version.project_id.clone(),
            version_id: version.id.clone(),
            title: project
                .map(|p| p.title.clone())
                .unwrap_or_else(|| version.name.clone()),
            icon_url: project.and_then(|p| p.icon_url.clone()),
            project_type: project
                .map(|p| p.project_type.clone())
                .unwrap_or_else(|| "mod".into()),
            files: vec![filename],
            installed_at: Some(chrono::Utc::now().to_rfc3339()),
        });
    }

    if !specs.is_empty() {
        download_many(http.clone(), specs, concurrency, "modpack", sink.clone()).await?;
    }

    // ---- extract overrides ------------------------------------------------
    sink(Progress::new("extract", "Applying modpack overrides…"));
    let pack = pack.to_path_buf();
    let dest = game_dir.to_path_buf();
    let overrides_dir = manifest.overrides.clone();
    tokio::task::spawn_blocking(move || extract_overrides(&pack, &dest, &overrides_dir))
        .await
        .map_err(|e| LauncherError::msg(format!("override extraction failed: {e}")))??;

    // The pack itself, so update checks and the content list know where the
    // instance came from. The caller replaces this with the API-backed record
    // when the pack has a real CurseForge project id.
    records.push(InstalledProject {
        project_id: String::new(),
        version_id: manifest.version.clone(),
        title: manifest.name.clone(),
        icon_url: None,
        project_type: "modpack".into(),
        files: vec![],
        installed_at: Some(chrono::Utc::now().to_rfc3339()),
    });

    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_json(loader: &str) -> String {
        format!(
            r#"{{
                "minecraft": {{
                    "version": "1.21.1",
                    "modLoaders": [{{ "id": "{loader}", "primary": true }}]
                }},
                "manifestType": "minecraftModpack",
                "manifestVersion": 1,
                "name": "Test Pack",
                "version": "1.2.3",
                "author": "Tester",
                "files": [
                    {{ "projectID": 388172, "fileID": 8893488, "required": true }},
                    {{ "projectID": 306612, "fileID": 9171702 }}
                ],
                "overrides": "overrides"
            }}"#
        )
    }

    #[test]
    fn parses_manifest_and_defaults() {
        let m: CfManifest = serde_json::from_str(&manifest_json("fabric-0.16.9")).unwrap();

        assert_eq!(m.name, "Test Pack");
        assert_eq!(m.minecraft.version, "1.21.1");
        assert_eq!(m.files.len(), 2);
        assert!(m.files[0].required);
        assert!(m.files[1].required); // missing flag defaults to required
        assert_eq!(m.files[0].project_id, 388172);
        assert_eq!(m.files[1].file_id, 9171702);
    }

    #[test]
    fn parses_every_loader() {
        for (id, kind) in [
            ("fabric-0.16.9", LoaderKind::Fabric),
            ("forge-47.2.0", LoaderKind::Forge),
            ("neoforge-21.1.172", LoaderKind::NeoForge),
            ("quilt-0.26.3-beta.2", LoaderKind::Quilt),
            ("minecraft-1.21.1", LoaderKind::Vanilla),
            ("rift-1.7.10", LoaderKind::Vanilla),
        ] {
            let m: CfManifest = serde_json::from_str(&manifest_json(id)).unwrap();
            let (parsed, version) = m.loader_info();
            assert_eq!(parsed, kind, "loader id {id}");
            if kind == LoaderKind::Vanilla {
                assert_eq!(version, None, "loader id {id}");
            } else {
                assert!(version.is_some(), "loader id {id} should carry a version");
            }
        }
    }

    #[test]
    fn loader_version_keeps_dashes() {
        let m: CfManifest = serde_json::from_str(&manifest_json("quilt-0.26.3-beta.2")).unwrap();
        let (_, version) = m.loader_info();
        assert_eq!(version.as_deref(), Some("0.26.3-beta.2"));
    }

    #[test]
    fn missing_modloaders_means_vanilla() {
        let json = r#"{
            "minecraft": { "version": "1.21.1", "modLoaders": [] },
            "name": "Bare",
            "files": []
        }"#;
        let m: CfManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.loader_info(), (LoaderKind::Vanilla, None));
        assert_eq!(m.overrides, "overrides");
    }
}
