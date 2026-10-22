//! Mojang version manifest and `version.json` handling.
//!
//! We deliberately model only the fields the launcher needs, but keep the
//! naming identical to Mojang's schema so the code maps 1:1 to the docs at
//! <https://minecraft.wiki/w/Client.json>.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::super::download::get_json;
use super::super::paths::{read_json, write_json, AppPaths};
use crate::error::{LauncherError, Result};

pub const VERSION_MANIFEST_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
pub const RESOURCES_BASE: &str = "https://resources.download.minecraft.net";

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionManifest {
    pub latest: LatestVersions,
    pub versions: Vec<VersionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
}

/// A single entry from the manifest. These are what the version picker shows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionSummary {
    pub id: String,
    /// `release`, `snapshot`, `old_beta` or `old_alpha`.
    #[serde(rename = "type")]
    pub version_type: String,
    pub url: String,
    pub time: String,
    pub release_time: String,
    #[serde(default)]
    pub sha1: String,
    #[serde(default)]
    pub compliance_level: u8,
}

// ---------------------------------------------------------------------------
// version.json
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VersionJson {
    pub id: String,
    #[serde(default)]
    pub main_class: String,
    #[serde(default)]
    pub assets: Option<String>,
    #[serde(default)]
    pub asset_index: Option<AssetIndex>,
    #[serde(default)]
    pub downloads: HashMap<String, DownloadEntry>,
    #[serde(default)]
    pub libraries: Vec<Library>,
    #[serde(default)]
    pub java_version: Option<JavaVersion>,
    #[serde(default)]
    pub arguments: Option<Arguments>,
    /// Pre-1.13 style argument string.
    #[serde(default)]
    pub minecraft_arguments: Option<String>,
    #[serde(default)]
    pub inherited_from: Option<String>,
    #[serde(rename = "type", default)]
    pub version_type: Option<String>,
    /// Mojang's log4j configuration. Its patched config files are what protect
    /// 1.7–1.18 against Log4Shell, so they are always applied when present.
    #[serde(default)]
    pub logging: Option<Logging>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Logging {
    #[serde(default)]
    pub client: Option<LoggingClient>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingClient {
    /// e.g. `-Dlog4j.configurationFile=${path}`
    pub argument: String,
    pub file: LoggingFile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingFile {
    pub id: String,
    #[serde(default)]
    pub sha1: String,
    #[serde(default)]
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersion {
    pub component: String,
    pub major_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetIndex {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    #[serde(default)]
    pub total_size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadEntry {
    #[serde(default)]
    pub sha1: String,
    #[serde(default)]
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<serde_json::Value>,
    #[serde(default)]
    pub jvm: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    /// Maven coordinate: `group:artifact:version[:classifier][@ext]`.
    pub name: String,
    #[serde(default)]
    pub downloads: Option<LibraryDownloads>,
    /// Maps an OS name to a classifier, for old-style natives.
    #[serde(default)]
    pub natives: Option<HashMap<String, String>>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub extract: Option<Extract>,
    /// Maven repository base url, used when `downloads` is absent (Fabric etc).
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LibraryDownloads {
    #[serde(default)]
    pub artifact: Option<DownloadEntry>,
    #[serde(default)]
    pub classifiers: Option<HashMap<String, DownloadEntry>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Extract {
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub action: String,
    #[serde(default)]
    pub os: Option<OsRule>,
    #[serde(default)]
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsRule {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
}

/// Features that can be toggled when evaluating rules.
#[derive(Debug, Clone, Default)]
pub struct Features {
    pub is_demo_user: bool,
    pub has_custom_resolution: bool,
    pub has_quick_plays_support: bool,
    pub is_quick_play_singleplayer: bool,
    pub is_quick_play_multiplayer: bool,
    pub is_quick_play_realms: bool,
}

impl Features {
    fn value(&self, name: &str) -> Option<bool> {
        match name {
            "is_demo_user" => Some(self.is_demo_user),
            "has_custom_resolution" => Some(self.has_custom_resolution),
            "has_quick_plays_support" => Some(self.has_quick_plays_support),
            "is_quick_play_singleplayer" => Some(self.is_quick_play_singleplayer),
            "is_quick_play_multiplayer" => Some(self.is_quick_play_multiplayer),
            "is_quick_play_realms" => Some(self.is_quick_play_realms),
            _ => None,
        }
    }
}

pub fn current_os_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

pub fn current_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86") {
        "x86"
    } else {
        "unknown"
    }
}

/// Evaluate a Mojang rule list. Empty rules mean "allowed".
pub fn rules_allow(rules: &[Rule], features: &Features) -> bool {
    if rules.is_empty() {
        return true;
    }

    // Later rules win, so keep the last matching action.
    let mut allowed = false;
    for rule in rules {
        if !rule_matches(rule, features) {
            continue;
        }
        allowed = rule.action == "allow";
    }
    allowed
}

fn rule_matches(rule: &Rule, features: &Features) -> bool {
    if let Some(os) = &rule.os {
        if let Some(name) = &os.name {
            if name != current_os_name() {
                return false;
            }
        }
        if let Some(arch) = &os.arch {
            // Mojang uses `x86` for 32-bit; be permissive for unknown arches.
            if arch != "x86" && arch != current_arch() {
                return false;
            }
        }
        // `os.version` is a regex; we only handle the common "not windows 10"
        // style cases by ignoring it. None of the current Mojang rules rely on
        // it when `name` is also present.
        let _ = &os.version;
    }

    if let Some(features_req) = &rule.features {
        for (key, expected) in features_req {
            match features.value(key) {
                Some(actual) if actual == *expected => {}
                // Unknown feature flags default to false.
                None if !*expected => {}
                _ => return false,
            }
        }
    }

    true
}

// ---------------------------------------------------------------------------
// Maven helpers
// ---------------------------------------------------------------------------

/// Split a maven coordinate into its parts.
///
/// `net.fabricmc:fabric-loader:0.15.0` → (`net.fabricmc/fabric-loader/0.15.0`, `jar`)
/// `foo:bar:1.0:natives-windows@jar` → (`foo/bar/1.0/bar-1.0-natives-windows.jar`, `jar`)
pub fn maven_path(name: &str) -> (String, String) {
    let (coordinate, ext) = match name.split_once('@') {
        Some((c, e)) => (c, e.to_string()),
        None => (name, "jar".to_string()),
    };

    let parts: Vec<&str> = coordinate.split(':').collect();
    let group = parts.first().copied().unwrap_or("unknown");
    let artifact = parts.get(1).copied().unwrap_or("artifact");
    let version = parts.get(2).copied().unwrap_or("0");
    let classifier = parts.get(3).copied();

    let file = match classifier {
        Some(c) => format!("{artifact}-{version}-{c}.{ext}"),
        None => format!("{artifact}-{version}.{ext}"),
    };

    (
        format!("{}/{}/{}/{}", group.replace('.', "/"), artifact, version, file),
        ext,
    )
}

// ---------------------------------------------------------------------------
// Fetching & caching
// ---------------------------------------------------------------------------

/// Download the version manifest, using a cached copy when present.
pub async fn fetch_manifest(
    http: &reqwest::Client,
    paths: &AppPaths,
    force: bool,
) -> Result<VersionManifest> {
    let cache = paths.meta_dir().join("version_manifest_v2.json");

    if !force {
        if let Some(manifest) = read_json::<VersionManifest>(&cache)? {
            return Ok(manifest);
        }
    }

    let manifest: VersionManifest = get_json(http, VERSION_MANIFEST_URL).await?;
    write_json(&cache, &manifest)?;
    Ok(manifest)
}

/// Fetch and cache a single `version.json` (vanilla only).
pub async fn fetch_version_json(
    http: &reqwest::Client,
    paths: &AppPaths,
    summary: &VersionSummary,
) -> Result<VersionJson> {
    let cache_dir = paths.versions_dir().join(&summary.id);
    let cache_file = cache_dir.join(format!("{}.json", summary.id));

    if let Some(version) = read_json::<VersionJson>(&cache_file)? {
        return Ok(version);
    }

    let version: VersionJson = get_json(http, &summary.url).await?;
    std::fs::create_dir_all(&cache_dir)?;
    write_json(&cache_file, &version)?;
    Ok(version)
}

/// Look up a manifest entry by version id.
pub fn find_version<'a>(
    manifest: &'a VersionManifest,
    id: &str,
) -> Result<&'a VersionSummary> {
    manifest
        .versions
        .iter()
        .find(|v| v.id == id)
        .ok_or_else(|| LauncherError::NotFound(format!("Minecraft version `{id}`")))
}

/// Path to the cached vanilla `version.json`.
pub fn cached_version_file(paths: &AppPaths, id: &str) -> PathBuf {
    paths.versions_dir().join(id).join(format!("{id}.json"))
}

// ---------------------------------------------------------------------------
// Inheritance
// ---------------------------------------------------------------------------

/// Merge a child version (e.g. a Fabric profile) onto its parent (vanilla).
///
/// Child libraries come first so they win classpath conflicts, matching the
/// behaviour of the official launcher.
pub fn merge_versions(parent: VersionJson, child: VersionJson) -> VersionJson {
    let mut libraries = child.libraries.clone();
    libraries.extend(parent.libraries.clone());

    let main_class = if child.main_class.is_empty() {
        parent.main_class.clone()
    } else {
        child.main_class.clone()
    };

    let arguments = match (parent.arguments, child.arguments) {
        (Some(p), Some(c)) => Some(Arguments {
            game: {
                let mut g = c.game;
                g.extend(p.game);
                g
            },
            jvm: {
                let mut j = c.jvm;
                j.extend(p.jvm);
                j
            },
        }),
        (Some(p), None) => Some(p),
        (None, Some(c)) => Some(c),
        (None, None) => None,
    };

    VersionJson {
        id: child.id,
        main_class,
        assets: child.assets.or(parent.assets),
        asset_index: child.asset_index.or(parent.asset_index),
        downloads: if child.downloads.is_empty() {
            parent.downloads
        } else {
            child.downloads
        },
        libraries,
        java_version: child.java_version.or(parent.java_version),
        arguments,
        minecraft_arguments: child.minecraft_arguments.or(parent.minecraft_arguments),
        inherited_from: None,
        version_type: child.version_type.or(parent.version_type),
        logging: child.logging.or(parent.logging),
    }
}
