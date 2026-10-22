//! Mojang version manifest, version JSON parsing, inheritance (for Fabric), rules,
//! libraries and assets.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::download::{fetch_bytes, sha1_hex, verify_sha1, write_file, Download};
use crate::paths::Paths;
use crate::state::AppState;

const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const LIBRARIES_URL: &str = "https://libraries.minecraft.net/";
const RESOURCES_URL: &str = "https://resources.download.minecraft.net";

// ---------- Version manifest ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionManifest {
    pub latest: LatestVersions,
    pub versions: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    pub release_time: String,
    #[serde(default)]
    pub sha1: Option<String>,
}

// ---------- Version JSON ----------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionJson {
    pub id: String,
    pub inherits_from: Option<String>,
    pub main_class: Option<String>,
    /// Pre-1.13 versions use a single argument string instead of `arguments`.
    pub minecraft_arguments: Option<String>,
    pub arguments: Option<Arguments>,
    pub asset_index: Option<AssetIndexRef>,
    pub downloads: Option<VersionDownloads>,
    #[serde(default)]
    pub libraries: Vec<Library>,
    pub java_version: Option<JavaVersion>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub logging: Option<Logging>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Argument>,
    #[serde(default)]
    pub jvm: Vec<Argument>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Argument {
    Plain(String),
    Conditional { rules: Vec<Rule>, value: ArgValue },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ArgValue {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub action: String,
    #[serde(default)]
    pub os: Option<OsRule>,
    #[serde(default)]
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OsRule {
    pub name: Option<String>,
    pub arch: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetIndexRef {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub sha1: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionDownloads {
    pub client: Option<Artifact>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Artifact {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersion {
    pub component: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Library {
    pub name: String,
    #[serde(default)]
    pub downloads: Option<LibraryDownloads>,
    /// Maven repository base URL (used by Fabric libraries).
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub rules: Option<Vec<Rule>>,
    #[serde(default)]
    pub natives: Option<HashMap<String, String>>,
    #[serde(default)]
    pub extract: Option<Extract>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LibraryDownloads {
    #[serde(default)]
    pub artifact: Option<Artifact>,
    #[serde(default)]
    pub classifiers: Option<HashMap<String, Artifact>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Extract {
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Logging {
    pub client: Option<LoggingClient>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingClient {
    pub argument: String,
    pub file: LoggingFile,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingFile {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

// ---------- Asset index ----------

#[derive(Debug, Clone, Deserialize)]
pub struct AssetIndex {
    pub objects: HashMap<String, AssetObject>,
    /// Pre-1.7.3 versions read assets from a "virtual" folder with real file names.
    #[serde(rename = "virtual", default)]
    pub is_virtual: bool,
    /// Pre-1.6 versions read assets from <game dir>/resources.
    #[serde(default)]
    pub map_to_resources: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

// ---------- Rules ----------

#[derive(Debug, Clone, Copy, Default)]
pub struct Features {
    pub quick_play_multiplayer: bool,
    pub custom_resolution: bool,
}

pub fn os_name() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

fn os_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    }
}

fn rule_matches(rule: &Rule, features: &Features) -> bool {
    if let Some(os) = &rule.os {
        if os.name.as_deref().is_some_and(|name| name != os_name()) {
            return false;
        }
        if os.arch.as_deref().is_some_and(|arch| arch != os_arch()) {
            return false;
        }
    }
    if let Some(wanted) = &rule.features {
        for (feature, value) in wanted {
            let have = match feature.as_str() {
                "is_quick_play_multiplayer" => features.quick_play_multiplayer,
                "has_custom_resolution" => features.custom_resolution,
                _ => false,
            };
            if have != *value {
                return false;
            }
        }
    }
    true
}

/// Mojang's rule semantics: no rules means allowed; otherwise the last matching rule wins.
pub fn rules_allow(rules: &[Rule], features: &Features) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    for rule in rules {
        if rule_matches(rule, features) {
            allowed = rule.action == "allow";
        }
    }
    allowed
}

pub fn collect_args(args: &[Argument], features: &Features) -> Vec<String> {
    let mut out = Vec::new();
    for arg in args {
        match arg {
            Argument::Plain(value) => out.push(value.clone()),
            Argument::Conditional { rules, value } => {
                if rules_allow(rules, features) {
                    match value {
                        ArgValue::One(v) => out.push(v.clone()),
                        ArgValue::Many(vs) => out.extend(vs.iter().cloned()),
                    }
                }
            }
        }
    }
    out
}

/// 1.20+ joins servers with `--quickPlayMultiplayer`; older versions use `--server`/`--port`.
pub fn supports_quick_play(version: &VersionJson) -> bool {
    version.arguments.as_ref().is_some_and(|args| {
        args.game.iter().any(|arg| match arg {
            Argument::Conditional { rules, .. } => rules.iter().any(|rule| {
                rule.features
                    .as_ref()
                    .is_some_and(|f| f.contains_key("is_quick_play_multiplayer"))
            }),
            Argument::Plain(_) => false,
        })
    })
}

// ---------- Manifest & version JSON loading ----------

/// Loads Mojang's version list, falling back to the cached copy when offline.
pub async fn get_manifest(state: &AppState) -> Result<VersionManifest> {
    let mut cached = state.manifest.lock().await;
    if let Some(manifest) = cached.as_ref() {
        return Ok(manifest.clone());
    }

    let cache_file = state.paths.cache().join("version_manifest_v2.json");
    let manifest: VersionManifest = match fetch_bytes(&state.http, MANIFEST_URL).await {
        Ok(bytes) => {
            let manifest = serde_json::from_slice(&bytes).context("Mojang's version list is invalid")?;
            let _ = write_file(&cache_file, &bytes).await;
            manifest
        }
        Err(e) => {
            let bytes = tokio::fs::read(&cache_file).await.map_err(|_| e)?;
            serde_json::from_slice(&bytes).context("the cached version list is invalid")?
        }
    };
    *cached = Some(manifest.clone());
    Ok(manifest)
}

/// Returns the vanilla version JSON, downloading it (or an updated copy) when needed.
pub async fn ensure_vanilla_json(state: &AppState, id: &str) -> Result<VersionJson> {
    let path = state.paths.version_json(id);
    let existing = tokio::fs::read(&path).await.ok();
    let entry = match get_manifest(state).await {
        Ok(manifest) => manifest.versions.into_iter().find(|v| v.id == id),
        Err(_) => None,
    };

    let bytes = match (existing, entry) {
        (Some(bytes), Some(entry))
            if entry
                .sha1
                .as_deref()
                .map_or(true, |sha1| sha1.eq_ignore_ascii_case(&sha1_hex(&bytes))) =>
        {
            bytes
        }
        (_, Some(entry)) => {
            let bytes = fetch_bytes(&state.http, &entry.url).await?;
            verify_sha1(&bytes, entry.sha1.as_deref(), &entry.url)?;
            write_file(&path, &bytes).await?;
            bytes
        }
        (Some(bytes), None) => bytes,
        (None, None) => bail!("Minecraft {id} was not found in Mojang's version list."),
    };
    serde_json::from_slice(&bytes).with_context(|| format!("the version file for {id} is invalid"))
}

pub struct ResolvedVersion {
    pub json: VersionJson,
    /// Version whose client jar is used (the vanilla parent for Fabric).
    pub jar_id: String,
}

/// Follows `inheritsFrom` and merges the chain into a single version.
pub async fn resolve(state: &AppState, top: VersionJson) -> Result<ResolvedVersion> {
    let mut chain = vec![top];
    while let Some(parent_id) = chain.last().and_then(|v| v.inherits_from.clone()) {
        if chain.len() > 8 {
            bail!("version inheritance is too deep");
        }
        chain.push(ensure_vanilla_json(state, &parent_id).await?);
    }

    let mut merged = chain.pop().expect("version chain is never empty");
    let jar_id = merged.id.clone();
    while let Some(child) = chain.pop() {
        merged = merge(child, merged);
    }
    Ok(ResolvedVersion { json: merged, jar_id })
}

fn merge(child: VersionJson, parent: VersionJson) -> VersionJson {
    // Child libraries come first so they win the de-duplication in `plan_libraries`.
    let mut libraries = child.libraries;
    libraries.extend(parent.libraries);

    let arguments = match (parent.arguments, child.arguments) {
        (Some(mut p), Some(c)) => {
            p.game.extend(c.game);
            p.jvm.extend(c.jvm);
            Some(p)
        }
        (p, c) => c.or(p),
    };

    VersionJson {
        id: child.id,
        inherits_from: None,
        main_class: child.main_class.or(parent.main_class),
        minecraft_arguments: child.minecraft_arguments.or(parent.minecraft_arguments),
        arguments,
        asset_index: child.asset_index.or(parent.asset_index),
        downloads: child.downloads.or(parent.downloads),
        libraries,
        java_version: child.java_version.or(parent.java_version),
        kind: child.kind.or(parent.kind),
        logging: child.logging.or(parent.logging),
    }
}

// ---------- Libraries ----------

#[derive(Debug, Clone)]
pub struct NativeJar {
    pub path: PathBuf,
    pub exclude: Vec<String>,
}

pub struct LibraryPlan {
    pub classpath: Vec<PathBuf>,
    pub natives: Vec<NativeJar>,
    pub downloads: Vec<Download>,
}

struct ResolvedArtifact {
    path: String,
    url: String,
    sha1: Option<String>,
    size: Option<u64>,
}

/// "group:artifact:version[:classifier][@ext]" -> "group/path/artifact/version/artifact-version[-classifier].ext"
pub fn maven_path(name: &str) -> Option<String> {
    let (coords, ext) = name.split_once('@').unwrap_or((name, "jar"));
    let parts: Vec<&str> = coords.split(':').collect();
    if parts.len() < 3 {
        return None;
    }
    let group = parts[0].replace('.', "/");
    let (artifact, version) = (parts[1], parts[2]);
    let file = match parts.get(3) {
        Some(classifier) => format!("{artifact}-{version}-{classifier}.{ext}"),
        None => format!("{artifact}-{version}.{ext}"),
    };
    Some(format!("{group}/{artifact}/{version}/{file}"))
}

/// group:artifact[:classifier], so a Fabric library replaces the vanilla copy of the same library.
fn library_key(name: &str) -> String {
    let coords = name.split('@').next().unwrap_or(name);
    let parts: Vec<&str> = coords.split(':').collect();
    match parts.as_slice() {
        [group, artifact, _version, classifier, ..] => format!("{group}:{artifact}:{classifier}"),
        [group, artifact, ..] => format!("{group}:{artifact}"),
        _ => name.to_string(),
    }
}

fn repo_base(lib: &Library) -> String {
    let base = lib.url.as_deref().unwrap_or(LIBRARIES_URL);
    if base.ends_with('/') {
        base.to_string()
    } else {
        format!("{base}/")
    }
}

fn main_artifact(lib: &Library) -> Option<ResolvedArtifact> {
    if let Some(downloads) = &lib.downloads {
        // Libraries with `downloads` but no `artifact` only ship natives.
        let artifact = downloads.artifact.as_ref()?;
        let path = artifact.path.clone().or_else(|| maven_path(&lib.name))?;
        return Some(ResolvedArtifact {
            path,
            url: artifact.url.clone(),
            sha1: artifact.sha1.clone(),
            size: artifact.size,
        });
    }
    if lib.natives.is_some() {
        return None;
    }
    let path = maven_path(&lib.name)?;
    Some(ResolvedArtifact {
        url: format!("{}{path}", repo_base(lib)),
        path,
        sha1: lib.sha1.clone(),
        size: lib.size,
    })
}

fn native_artifact(lib: &Library) -> Option<ResolvedArtifact> {
    let classifier = lib.natives.as_ref()?.get(os_name())?;
    let classifier = classifier.replace("${arch}", if cfg!(target_pointer_width = "64") { "64" } else { "32" });
    let native_name = format!("{}:{classifier}", lib.name);

    let listed = lib
        .downloads
        .as_ref()
        .and_then(|d| d.classifiers.as_ref())
        .and_then(|c| c.get(&classifier));
    if let Some(artifact) = listed {
        return Some(ResolvedArtifact {
            path: artifact.path.clone().or_else(|| maven_path(&native_name))?,
            url: artifact.url.clone(),
            sha1: artifact.sha1.clone(),
            size: artifact.size,
        });
    }
    let path = maven_path(&native_name)?;
    Some(ResolvedArtifact {
        url: format!("{}{path}", repo_base(lib)),
        path,
        sha1: None,
        size: None,
    })
}

pub fn plan_libraries(paths: &Paths, libraries: &[Library], features: &Features) -> LibraryPlan {
    let root = paths.libraries();
    let mut plan = LibraryPlan {
        classpath: Vec::new(),
        natives: Vec::new(),
        downloads: Vec::new(),
    };
    let mut seen = HashSet::new();

    for lib in libraries {
        // Filter by rules before de-duplicating: some versions list the same library twice
        // with different versions for different operating systems.
        if !rules_allow(lib.rules.as_deref().unwrap_or(&[]), features) {
            continue;
        }
        if !seen.insert(library_key(&lib.name)) {
            continue;
        }

        if let Some(artifact) = main_artifact(lib) {
            let path = root.join(&artifact.path);
            if !artifact.url.is_empty() {
                plan.downloads.push(Download {
                    url: artifact.url,
                    path: path.clone(),
                    sha1: artifact.sha1,
                    size: artifact.size,
                    executable: false,
                });
            }
            plan.classpath.push(path);
        }

        if let Some(artifact) = native_artifact(lib) {
            let path = root.join(&artifact.path);
            if !artifact.url.is_empty() {
                plan.downloads.push(Download {
                    url: artifact.url,
                    path: path.clone(),
                    sha1: artifact.sha1,
                    size: artifact.size,
                    executable: false,
                });
            }
            plan.natives.push(NativeJar {
                path,
                exclude: lib.extract.as_ref().map(|e| e.exclude.clone()).unwrap_or_default(),
            });
        }
    }
    plan
}

// ---------- Assets ----------

pub async fn ensure_asset_index(state: &AppState, index: &AssetIndexRef) -> Result<AssetIndex> {
    let path = state.paths.assets().join("indexes").join(format!("{}.json", index.id));
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes)
            if index
                .sha1
                .as_deref()
                .map_or(true, |sha1| sha1.eq_ignore_ascii_case(&sha1_hex(&bytes))) =>
        {
            bytes
        }
        _ => {
            let bytes = fetch_bytes(&state.http, &index.url).await?;
            verify_sha1(&bytes, index.sha1.as_deref(), &index.url)?;
            write_file(&path, &bytes).await?;
            bytes
        }
    };
    serde_json::from_slice(&bytes).context("the asset index is invalid")
}

pub fn asset_downloads(paths: &Paths, index: &AssetIndex) -> Vec<Download> {
    let objects = paths.assets().join("objects");
    index
        .objects
        .values()
        .filter_map(|object| {
            let prefix = object.hash.get(..2)?;
            Some(Download {
                url: format!("{RESOURCES_URL}/{prefix}/{}", object.hash),
                path: objects.join(prefix).join(&object.hash),
                sha1: Some(object.hash.clone()),
                size: Some(object.size),
                executable: false,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(json: &str) -> Rule {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn maven_paths() {
        assert_eq!(
            maven_path("org.lwjgl:lwjgl:3.3.1").as_deref(),
            Some("org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1.jar")
        );
        assert_eq!(
            maven_path("org.lwjgl:lwjgl:3.3.1:natives-windows").as_deref(),
            Some("org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1-natives-windows.jar")
        );
        assert_eq!(
            maven_path("com.example:thing:1.0@zip").as_deref(),
            Some("com/example/thing/1.0/thing-1.0.zip")
        );
        assert_eq!(maven_path("broken"), None);
    }

    #[test]
    fn library_keys_ignore_version_but_keep_classifier() {
        assert_eq!(library_key("org.ow2.asm:asm:9.6"), library_key("org.ow2.asm:asm:9.8"));
        assert_ne!(
            library_key("org.lwjgl:lwjgl:3.3.1"),
            library_key("org.lwjgl:lwjgl:3.3.1:natives-windows")
        );
    }

    #[test]
    fn rules() {
        let features = Features::default();
        assert!(rules_allow(&[], &features));

        let other_os = if os_name() == "osx" { "linux" } else { "osx" };
        let disallow_other = [
            rule(r#"{"action":"allow"}"#),
            rule(&format!(r#"{{"action":"disallow","os":{{"name":"{other_os}"}}}}"#)),
        ];
        assert!(rules_allow(&disallow_other, &features));

        let only_other = [rule(&format!(r#"{{"action":"allow","os":{{"name":"{other_os}"}}}}"#))];
        assert!(!rules_allow(&only_other, &features));

        let quick_play = [rule(r#"{"action":"allow","features":{"is_quick_play_multiplayer":true}}"#)];
        assert!(!rules_allow(&quick_play, &features));
        let with_quick_play = Features { quick_play_multiplayer: true, ..Features::default() };
        assert!(rules_allow(&quick_play, &with_quick_play));
    }

    #[test]
    fn arguments_parse_and_filter() {
        let args: Vec<Argument> = serde_json::from_str(
            r#"["--username", "${auth_player_name}",
                {"rules":[{"action":"allow","features":{"is_demo_user":true}}],"value":"--demo"},
                {"rules":[{"action":"allow","features":{"is_quick_play_multiplayer":true}}],"value":["--quickPlayMultiplayer","${quickPlayMultiplayer}"]}]"#,
        )
        .unwrap();
        let plain = collect_args(&args, &Features::default());
        assert_eq!(plain, ["--username", "${auth_player_name}"]);
        let joined = collect_args(&args, &Features { quick_play_multiplayer: true, ..Features::default() });
        assert_eq!(joined.len(), 4);
        assert_eq!(joined[2], "--quickPlayMultiplayer");
    }
}

/// Copies assets to their real names for old versions that don't understand the object store.
pub fn copy_assets(paths: &Paths, index: &AssetIndex, target: &Path) -> Result<()> {
    let objects = paths.assets().join("objects");
    for (name, object) in &index.objects {
        let Some(prefix) = object.hash.get(..2) else { continue };
        let destination = target.join(name);
        let up_to_date = std::fs::metadata(&destination)
            .map(|meta| meta.len() == object.size)
            .unwrap_or(false);
        if up_to_date {
            continue;
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(objects.join(prefix).join(&object.hash), &destination)
            .with_context(|| format!("could not copy asset {name}"))?;
    }
    Ok(())
}
