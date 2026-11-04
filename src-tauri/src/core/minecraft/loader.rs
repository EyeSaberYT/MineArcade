//! Mod loader resolution and installation.
//!
//! * **Fabric / Quilt** publish a complete `version.json` *profile* which we can
//!   merge onto the vanilla version at launch time. Fast, no Java required.
//! * **Forge / NeoForge** ship an installer jar which must be executed with
//!   Java. The installer writes a new version profile plus the loader libraries
//!   into the shared meta folder, which is laid out like `.minecraft`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::manifest::VersionJson;
use crate::api::modrinth::urlencode;
use crate::core::download::{download_one, get_json, DownloadSpec, ProgressSink};
use crate::core::instance::LoaderKind;
use crate::core::paths::{read_json, write_json, AppPaths};
use crate::error::{LauncherError, Result};

const FABRIC_META: &str = "https://meta.fabricmc.net/v2";
const QUILT_META: &str = "https://meta.quiltmc.org/v3";
const FORGE_MAVEN: &str = "https://maven.minecraftforge.net";
const NEOFORGE_MAVEN: &str = "https://maven.neoforged.net/releases";

/// One selectable loader build.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderVersion {
    pub version: String,
    pub stable: bool,
    pub game_version: String,
}

// ---------------------------------------------------------------------------
// Version listing
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct MetaLoaderEntry {
    #[serde(default)]
    version: String,
    #[serde(default)]
    stable: bool,
}

#[derive(Deserialize)]
struct FabricLoaderForGame {
    loader: MetaLoaderEntry,
}

/// List available loader builds for a game version.
pub async fn list_loader_versions(
    http: &reqwest::Client,
    kind: LoaderKind,
    game_version: &str,
) -> Result<Vec<LoaderVersion>> {
    validate_version("Minecraft version", game_version)?;
    match kind {
        LoaderKind::Vanilla => Ok(vec![]),
        LoaderKind::Fabric => {
            let url = format!("{FABRIC_META}/versions/loader/{}", urlencode(game_version));
            let raw: Vec<FabricLoaderForGame> = get_json(http, &url).await?;
            Ok(raw
                .into_iter()
                .map(|e| LoaderVersion {
                    version: e.loader.version,
                    stable: e.loader.stable,
                    game_version: game_version.to_string(),
                })
                .collect())
        }
        LoaderKind::Quilt => {
            let url = format!("{QUILT_META}/versions/loader/{}", urlencode(game_version));
            let raw: Vec<FabricLoaderForGame> = get_json(http, &url).await?;
            Ok(raw
                .into_iter()
                .map(|e| LoaderVersion {
                    version: e.loader.version,
                    stable: e.loader.stable,
                    game_version: game_version.to_string(),
                })
                .collect())
        }
        LoaderKind::Forge => {
            let promotions: serde_json::Value =
                get_json(http, &format!("{FORGE_MAVEN}/net/minecraftforge/forge/promotions_slim.json"))
                    .await?;
            let promos = promotions
                .get("promos")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            let mut out = vec![];
            for (key, value) in promos.as_object().into_iter().flatten() {
                // Keys look like "1.20.1-latest" / "1.20.1-recommended".
                if let Some(game) = key.strip_suffix("-latest") {
                    if game == game_version {
                        if let Some(version) = value.as_str() {
                            out.push(LoaderVersion {
                                version: version.to_string(),
                                stable: false,
                                game_version: game.to_string(),
                            });
                        }
                    }
                } else if let Some(game) = key.strip_suffix("-recommended") {
                    if game == game_version {
                        if let Some(version) = value.as_str() {
                            out.push(LoaderVersion {
                                version: version.to_string(),
                                stable: true,
                                game_version: game.to_string(),
                            });
                        }
                    }
                }
            }
            Ok(out)
        }
        LoaderKind::NeoForge => {
            let Some(prefix) = neoforge_prefix(game_version) else {
                return Ok(vec![]);
            };
            let xml = http
                .get(format!("{NEOFORGE_MAVEN}/net/neoforged/neoforge/maven-metadata.xml"))
                .send()
                .await?
                .text()
                .await
                .unwrap_or_default();
            Ok(extract_xml_tags(&xml, "version")
                .into_iter()
                .filter(|v| v.starts_with(&prefix))
                .map(|v| LoaderVersion {
                    version: v,
                    stable: true,
                    game_version: game_version.to_string(),
                })
                .collect())
        }
    }
}

/// The `net.neoforged:neoforge` version prefix for a game version: 1.20.4 →
/// `20.4.`, 1.21 → `21.0.`, 1.21.1 → `21.1.`, and for year-based releases
/// 26.1 → `26.1.0.`, 26.1.2 → `26.1.2.`.
///
/// `None` when that artifact has no builds for the version: snapshots, and
/// 1.20.1 and older (NeoForge's 1.20.1 builds were `net.neoforged:forge`
/// re-releases of Forge 47, which we don't offer).
fn neoforge_prefix(game_version: &str) -> Option<String> {
    let numeric = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    let parts: Vec<&str> = game_version.split('.').collect();
    if !parts.iter().all(|p| numeric(p)) {
        return None;
    }
    match parts.as_slice() {
        ["1", major, rest @ ..] if rest.len() <= 1 => {
            let minor = rest.first().copied().unwrap_or("0");
            let (major_n, minor_n): (u32, u32) = (major.parse().ok()?, minor.parse().ok()?);
            if major_n < 20 || (major_n == 20 && minor_n < 2) {
                return None;
            }
            Some(format!("{major}.{minor}."))
        }
        [year, drop, rest @ ..] if *year != "1" && rest.len() <= 1 => {
            let hotfix = rest.first().copied().unwrap_or("0");
            Some(format!("{year}.{drop}.{hotfix}."))
        }
        _ => None,
    }
}

/// Loader and game versions end up in download URLs and in
/// `meta/versions/<id>`, so only accept the characters real versions use.
pub fn validate_version(label: &str, value: &str) -> Result<()> {
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-');
    if value.is_empty() || value.contains("..") || !value.chars().all(allowed) {
        return Err(LauncherError::InvalidData(format!("{label} `{value}` isn't a valid version.")));
    }
    Ok(())
}

/// Tiny XML tag extractor so we don't pull in a full XML parser for one call.
fn extract_xml_tags(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = vec![];
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        match after.find(&close) {
            Some(end) => {
                out.push(after[..end].trim().to_string());
                rest = &after[end + close.len()..];
            }
            None => break,
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Profile ids
// ---------------------------------------------------------------------------

/// The version id a loader profile is stored under in `meta/versions`.
pub fn profile_id(kind: LoaderKind, game_version: &str, loader_version: &str) -> String {
    match kind {
        LoaderKind::Vanilla => game_version.to_string(),
        LoaderKind::Fabric => format!("fabric-loader-{loader_version}-{game_version}"),
        LoaderKind::Quilt => format!("quilt-loader-{loader_version}-{game_version}"),
        LoaderKind::Forge => format!("{game_version}-forge-{loader_version}"),
        LoaderKind::NeoForge => format!("neoforge-{loader_version}"),
    }
}

/// Where a loader profile json lives inside `meta/versions`.
pub fn profile_file(paths: &AppPaths, id: &str) -> PathBuf {
    paths.versions_dir().join(id).join(format!("{id}.json"))
}

/// True when the loader profile has already been installed.
pub fn is_installed(paths: &AppPaths, id: &str) -> bool {
    profile_file(paths, id).exists()
}

// ---------------------------------------------------------------------------
// Installation
// ---------------------------------------------------------------------------

/// Download (and for Forge-family, execute) a loader profile.
///
/// Returns the profile id. Idempotent: repeated calls are cheap. Everything is
/// installed into the shared meta folder; `_game_dir` is no longer written to.
pub async fn install_loader(
    http: &reqwest::Client,
    paths: &AppPaths,
    _game_dir: &Path,
    kind: LoaderKind,
    game_version: &str,
    loader_version: &str,
    java: &Path,
    sink: ProgressSink,
) -> Result<String> {
    validate_version("Minecraft version", game_version)?;
    validate_version("Loader version", loader_version)?;
    let id = profile_id(kind, game_version, loader_version);
    if is_installed(paths, &id) {
        return Ok(id);
    }

    match kind {
        LoaderKind::Vanilla => Ok(id),
        LoaderKind::Fabric | LoaderKind::Quilt => {
            let base = if kind == LoaderKind::Fabric {
                FABRIC_META
            } else {
                QUILT_META
            };
            let url = format!(
                "{base}/versions/loader/{}/{}/profile/json",
                urlencode(game_version),
                urlencode(loader_version)
            );
            let profile: VersionJson = get_json(http, &url).await?;
            // The meta server supplies its own id; keep ours so caching is stable.
            let mut profile = profile;
            profile.id = id.clone();
            write_json(&profile_file(paths, &id), &profile)?;
            Ok(id)
        }
        LoaderKind::Forge | LoaderKind::NeoForge => {
            run_installer(
                http,
                paths,
                kind,
                game_version,
                loader_version,
                java,
                sink,
            )
            .await?;
            Ok(id)
        }
    }
}

/// Forge / NeoForge: download the installer jar and run it headlessly.
#[allow(clippy::too_many_arguments)]
async fn run_installer(
    http: &reqwest::Client,
    paths: &AppPaths,
    kind: LoaderKind,
    game_version: &str,
    loader_version: &str,
    java: &Path,
    sink: ProgressSink,
) -> Result<()> {
    let (url, file_name) = match kind {
        LoaderKind::Forge => {
            let maven = forge_maven_version(http, game_version, loader_version).await;
            validate_version("Forge version", &maven)?;
            let encoded = urlencode(&maven);
            (
                format!("{FORGE_MAVEN}/net/minecraftforge/forge/{encoded}/forge-{encoded}-installer.jar"),
                format!("forge-{maven}-installer.jar"),
            )
        }
        LoaderKind::NeoForge => {
            match neoforge_prefix(game_version) {
                Some(prefix) if loader_version.starts_with(&prefix) => {}
                _ => {
                    return Err(LauncherError::Unsupported(format!(
                        "NeoForge {loader_version} for Minecraft {game_version}"
                    )))
                }
            }
            let encoded = urlencode(loader_version);
            (
                format!(
                    "{NEOFORGE_MAVEN}/net/neoforged/neoforge/{encoded}/neoforge-{encoded}-installer.jar"
                ),
                format!("neoforge-{loader_version}-installer.jar"),
            )
        }
        _ => unreachable!(),
    };

    let installer = paths.cache_dir().join(file_name);
    sink(crate::core::download::Progress::new(
        "install",
        format!("Fetching the {} installer…", kind.as_str()),
    ));
    download_one(http, &DownloadSpec::new(url, &installer), &|_| {}).await?;

    let id = profile_id(kind, game_version, loader_version);

    // Pre-1.13 Forge ships the legacy installer, which can't install a client
    // headlessly. Its install_profile.json already holds the finished profile
    // and the Forge jar, so install from those directly (as Prism/MultiMC do).
    if kind == LoaderKind::Forge {
        let (jar, libraries) = (installer.clone(), paths.libraries_dir());
        let legacy = tokio::task::spawn_blocking(move || legacy_forge_profile(&jar, &libraries))
            .await
            .map_err(|e| LauncherError::msg(format!("Couldn't read the Forge installer: {e}")))??;
        if let Some(mut profile) = legacy {
            profile.id = id.clone();
            write_json(&profile_file(paths, &id), &profile)?;
            return Ok(());
        }
    }

    // The installer treats its target like a `.minecraft` folder: it writes
    // `versions/<id>/<id>.json`, the vanilla jar and every library (including
    // ones it generates, which can't be downloaded later). Our meta folder has
    // that exact layout, so install straight into it and the launch classpath,
    // built from `libraries_dir()`, finds everything. The installer refuses to
    // run without a launcher profile file there.
    let root = paths.meta_dir();
    let launcher_profiles = root.join("launcher_profiles.json");
    if !launcher_profiles.exists() {
        write_json(&launcher_profiles, &serde_json::json!({ "profiles": {} }))?;
    }

    sink(crate::core::download::Progress::new(
        "install",
        format!("Running the {} installer…", kind.as_str()),
    ));

    // The installer is a plain Java program. `--installClient <dir>` mirrors
    // what the official launcher does when you double-click it.
    let output = crate::core::no_console(&mut tokio::process::Command::new(java))
        .arg("-jar")
        .arg(&installer)
        .arg("--installClient")
        .arg(&root)
        .output()
        .await
        .map_err(|e| LauncherError::msg(format!("Couldn't run the installer: {e}")))?;

    // The installer writes the profile before generating libraries; drop it
    // on failure so a half-finished install isn't treated as installed.
    let installed = profile_file(paths, &id);
    if !output.status.success() {
        let _ = std::fs::remove_file(&installed);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(LauncherError::Launch(format!(
            "The {} installer failed.\n{}\n{}",
            kind.as_str(),
            stdout.lines().last().unwrap_or(""),
            stderr.lines().last().unwrap_or("")
        )));
    }

    // Re-save the profile in our own shape (and make sure it parses).
    if !installed.exists() {
        return Err(LauncherError::msg(format!(
            "The installer finished but produced no profile at {}",
            installed.display()
        )));
    }
    let profile: VersionJson = match read_json(&installed) {
        Ok(Some(profile)) => profile,
        result => {
            let _ = std::fs::remove_file(&installed);
            result?;
            return Err(LauncherError::msg("The installer profile was empty."));
        }
    };
    write_json(&installed, &profile)?;
    Ok(())
}

/// Forge's Maven name for a build. Builds before 1.10 carry a branch suffix
/// (e.g. `1.8.9-11.15.1.2318-1.8.9`), so look the real name up in Forge's
/// metadata and fall back to the plain `<game>-<forge>` form.
async fn forge_maven_version(http: &reqwest::Client, game_version: &str, loader_version: &str) -> String {
    let plain = format!("{game_version}-{loader_version}");
    let xml = match http
        .get(format!("{FORGE_MAVEN}/net/minecraftforge/forge/maven-metadata.xml"))
        .send()
        .await
    {
        Ok(response) => response.text().await.unwrap_or_default(),
        Err(_) => return plain,
    };
    pick_forge_version(&extract_xml_tags(&xml, "version"), &plain)
}

fn pick_forge_version(versions: &[String], plain: &str) -> String {
    let suffixed = format!("{plain}-");
    versions
        .iter()
        .find(|v| *v == plain)
        .or_else(|| versions.iter().find(|v| v.starts_with(&suffixed)))
        .cloned()
        .unwrap_or_else(|| plain.to_string())
}

/// Read a legacy (pre-1.13) Forge installer: extract its Forge jar into the
/// libraries folder and return the version profile it carries. Returns `None`
/// for modern installers, which must be run instead.
fn legacy_forge_profile(installer: &Path, libraries_dir: &Path) -> Result<Option<VersionJson>> {
    use std::io::Read;

    let mut archive = zip::ZipArchive::new(std::fs::File::open(installer)?)?;
    let profile: serde_json::Value = {
        let mut entry = match archive.by_name("install_profile.json") {
            Ok(entry) => entry,
            Err(_) => return Ok(None),
        };
        let mut text = String::new();
        entry.read_to_string(&mut text)?;
        serde_json::from_str(&text)?
    };
    let Some(version_info) = profile.get("versionInfo") else {
        return Ok(None);
    };
    let install = &profile["install"];
    let (Some(jar_entry), Some(coordinate)) = (install["filePath"].as_str(), install["path"].as_str()) else {
        return Err(LauncherError::msg("The Forge installer's profile is incomplete."));
    };

    // The universal jar is only published inside the installer, so put it
    // where the profile's `net.minecraftforge:forge` library expects it.
    let target = libraries_dir.join(super::manifest::maven_path(coordinate).0);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut source = archive
        .by_name(jar_entry)
        .map_err(|_| LauncherError::msg(format!("The Forge installer is missing {jar_entry}.")))?;
    let tmp = target.with_extension("part");
    std::io::copy(&mut source, &mut std::fs::File::create(&tmp)?)?;
    std::fs::rename(&tmp, &target)?;

    // Like the vanilla launcher, skip only libraries marked `clientreq: false`
    // (server-only, e.g. jline).
    let mut version_info = version_info.clone();
    if let Some(libraries) = version_info.get_mut("libraries").and_then(|l| l.as_array_mut()) {
        libraries.retain(|library| library.get("clientreq").and_then(|v| v.as_bool()) != Some(false));
    }
    Ok(Some(serde_json::from_value(version_info)?))
}

#[cfg(test)]
mod forge_tests {
    use super::*;

    #[test]
    fn finds_suffixed_forge_builds() {
        let versions: Vec<String> = ["1.8.9-11.15.1.2318-1.8.9", "1.12.2-14.23.5.2860", "1.7.10-10.13.4.1614-1.7.10"]
            .iter()
            .map(|v| v.to_string())
            .collect();
        assert_eq!(pick_forge_version(&versions, "1.8.9-11.15.1.2318"), "1.8.9-11.15.1.2318-1.8.9");
        assert_eq!(pick_forge_version(&versions, "1.12.2-14.23.5.2860"), "1.12.2-14.23.5.2860");
        assert_eq!(pick_forge_version(&versions, "1.20.1-47.2.0"), "1.20.1-47.2.0");
    }

    #[test]
    fn neoforge_prefixes_match_only_their_game_version() {
        assert_eq!(neoforge_prefix("1.20.4").as_deref(), Some("20.4."));
        assert_eq!(neoforge_prefix("1.21").as_deref(), Some("21.0."));
        assert_eq!(neoforge_prefix("1.21.1").as_deref(), Some("21.1."));
        assert_eq!(neoforge_prefix("1.21.10").as_deref(), Some("21.10."));
        assert_eq!(neoforge_prefix("26.1").as_deref(), Some("26.1.0."));
        assert_eq!(neoforge_prefix("26.1.2").as_deref(), Some("26.1.2."));
        assert_eq!(neoforge_prefix("1.20.1"), None);
        assert_eq!(neoforge_prefix("1.19.2"), None);
        assert_eq!(neoforge_prefix("25w14craftmine"), None);

        let prefix = neoforge_prefix("1.21").unwrap();
        assert!("21.0.167".starts_with(&prefix));
        assert!(!"21.1.77".starts_with(&prefix));
        assert!(!"21.10.5".starts_with(&prefix));
    }

    #[test]
    fn rejects_unsafe_versions() {
        for ok in ["1.20.1", "47.2.0", "0.15.0+build.1", "21.0.0-beta", "1.8.9-11.15.1.2318-1.8.9"] {
            assert!(validate_version("v", ok).is_ok(), "{ok}");
        }
        for bad in ["", "..", "1.20/../../x", "a\\b", "1.20?x=1", "1 2", "a..b"] {
            assert!(validate_version("v", bad).is_err(), "{bad}");
        }
    }

    /// Needs the real 1.8.9 installer: set FORGE_LEGACY_INSTALLER to its path.
    #[test]
    fn reads_a_legacy_installer() {
        let Ok(installer) = std::env::var("FORGE_LEGACY_INSTALLER") else { return };
        let libs = std::env::temp_dir().join(format!("orbit-launcher-forge-libs-{}", std::process::id()));
        let profile = legacy_forge_profile(Path::new(&installer), &libs).unwrap().expect("legacy profile");
        assert_eq!(profile.main_class, "net.minecraft.launchwrapper.Launch");
        assert_eq!(profile.inherited_from.as_deref(), Some("1.8.9"));
        assert!(profile.libraries.iter().all(|l| !l.name.starts_with("jline:")), "server-only libs are dropped");
        assert!(libs
            .join("net/minecraftforge/forge/1.8.9-11.15.1.2318-1.8.9/forge-1.8.9-11.15.1.2318-1.8.9.jar")
            .is_file());
        let _ = std::fs::remove_dir_all(&libs);
    }
}
