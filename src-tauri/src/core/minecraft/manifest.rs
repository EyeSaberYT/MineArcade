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
    /// The vanilla version a loader profile builds on. Mojang and Forge call it
    /// `inheritsFrom`; earlier builds of this launcher saved `inheritedFrom`.
    #[serde(default, rename = "inheritsFrom", alias = "inheritedFrom")]
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
    std::env::consts::ARCH
}

/// Does a rule's `os.arch` name the architecture `current` (a Rust
/// `std::env::consts::ARCH`)? Mojang matches it against Java's `os.arch`, where
/// `x86` is 32-bit only and 64-bit Intel reports `amd64`.
fn arch_matches(rule_arch: &str, current: &str) -> bool {
    match rule_arch {
        "x86" => current == "x86",
        "x86_64" | "amd64" => current == "x86_64",
        "arm64" | "aarch64" => current == "aarch64",
        other => other == current,
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
            if !arch_matches(arch, current_arch()) {
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

/// A library's identity without its version: `group:artifact[:classifier]` (and
/// `@extension`), so `org.ow2.asm:asm:9.6` and `org.ow2.asm:asm:9.7` are one library
/// while `lwjgl:3.3.3` and `lwjgl:3.3.3:natives-windows` are two.
fn library_key(name: &str) -> String {
    let (coords, ext) = match name.split_once('@') {
        Some((coords, ext)) => (coords, Some(ext)),
        None => (name, None),
    };
    let parts: Vec<&str> = coords.split(':').collect();
    let mut key = match parts.as_slice() {
        [group, artifact, _version, classifier, ..] => format!("{group}:{artifact}:{classifier}"),
        [group, artifact, ..] => format!("{group}:{artifact}"),
        _ => coords.to_string(),
    };
    if let Some(ext) = ext {
        key.push('@');
        key.push_str(ext);
    }
    key
}

/// Merge a child version (e.g. a Fabric profile) onto its parent (vanilla).
///
/// Child libraries come first, and replace the parent's copy of the same library
/// (in any version), matching the behaviour of the official launcher. Both on the
/// classpath would load twice: NeoForge refuses to start ("Duplicate key
/// ...commons-lang3-3.14.0.jar") and newer copies would mix with older ones.
pub fn merge_versions(parent: VersionJson, child: VersionJson) -> VersionJson {
    let mut libraries = child.libraries.clone();
    let overridden: std::collections::HashSet<String> = child.libraries.iter().map(|l| library_key(&l.name)).collect();
    libraries.extend(parent.libraries.iter().filter(|l| !overridden.contains(&library_key(&l.name))).cloned());

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

#[cfg(test)]
mod rule_tests {
    use super::*;

    #[test]
    fn loader_libraries_replace_the_games_copies() {
        let version = |id: &str, libraries: &[&str]| -> VersionJson {
            serde_json::from_value(serde_json::json!({
                "id": id,
                "libraries": libraries.iter().map(|name| serde_json::json!({ "name": name })).collect::<Vec<_>>(),
            }))
            .unwrap()
        };
        let game = version("1.21.1", &[
            "org.apache.commons:commons-lang3:3.14.0",
            "org.ow2.asm:asm:9.6",
            "org.lwjgl:lwjgl:3.3.3",
            "org.lwjgl:lwjgl:3.3.3:natives-windows",
            "com.mojang:brigadier:1.3.10",
        ]);
        let loader = version("neoforge-21.1.265", &["org.apache.commons:commons-lang3:3.14.0", "org.ow2.asm:asm:9.7", "net.neoforged:neoforge:21.1.265:universal"]);
        let names: Vec<String> = merge_versions(game, loader).libraries.into_iter().map(|l| l.name).collect();
        assert_eq!(names, [
            "org.apache.commons:commons-lang3:3.14.0",
            "org.ow2.asm:asm:9.7",
            "net.neoforged:neoforge:21.1.265:universal",
            "org.lwjgl:lwjgl:3.3.3",
            "org.lwjgl:lwjgl:3.3.3:natives-windows",
            "com.mojang:brigadier:1.3.10",
        ]);
        assert_eq!(library_key("de.oceanlabs.mcp:mcp_config:1.21.1-20240808@zip"), "de.oceanlabs.mcp:mcp_config@zip");
    }

    #[test]
    fn x86_means_32_bit_only() {
        assert!(arch_matches("x86", "x86"));
        assert!(!arch_matches("x86", "x86_64"));
        assert!(!arch_matches("x86", "aarch64"));
        assert!(arch_matches("x86_64", "x86_64"));
        assert!(arch_matches("amd64", "x86_64"));
        assert!(arch_matches("arm64", "aarch64"));
        assert!(!arch_matches("arm64", "x86_64"));
    }

    #[test]
    fn arch_rule_follows_the_host() {
        let rules: Vec<Rule> = serde_json::from_value(serde_json::json!([
            { "action": "allow", "os": { "arch": "x86" } }
        ]))
        .unwrap();
        assert_eq!(rules_allow(&rules, &Features::default()), std::env::consts::ARCH == "x86");
    }
}
