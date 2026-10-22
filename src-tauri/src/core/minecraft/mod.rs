//! Vanilla + loader installation and launch command construction.
//!
//! [`resolve`] produces the final [`VersionJson`] for an instance (merging the
//! loader profile onto the vanilla version), [`ensure_files`] makes sure every
//! jar/asset/native is on disk, and [`build_command`] turns all of it into an
//! actual `java …` invocation.

pub mod loader;
pub mod manifest;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use manifest::{
    current_os_name, find_version, maven_path, merge_versions, rules_allow, Arguments, Features,
    VersionJson,
};

use crate::core::download::{download_many, DownloadSpec, Progress, ProgressSink};
use crate::core::instance::{LoaderKind, QuickPlay};
use crate::core::paths::{read_json, write_json, AppPaths};
use crate::error::{LauncherError, Result};

/// Everything needed to build a launch command.
#[derive(Debug, Clone)]
pub struct LaunchContext {
    pub version: VersionJson,
    pub base_version: String,
    pub game_dir: PathBuf,
    pub assets_dir: PathBuf,
    pub asset_index: String,
    pub natives_dir: PathBuf,
    pub classpath: Vec<PathBuf>,
    pub username: String,
    pub uuid: String,
    pub access_token: String,
    pub user_type: String,
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub memory_min: u32,
    pub memory_max: u32,
    pub extra_jvm_args: Vec<String>,
    pub extra_game_args: Vec<String>,
    pub quick_play: QuickPlay,
    /// `-Dlog4j.configurationFile=…` pointing at Mojang's patched config.
    pub logging_arg: Option<String>,
}

/// Result of preparing an instance for launch.
#[derive(Debug, Clone)]
pub struct InstalledVersion {
    pub version: VersionJson,
    pub classpath: Vec<PathBuf>,
    pub natives_dir: PathBuf,
    pub asset_index: String,
    pub main_class: String,
    pub java_major: u32,
    /// `-Dlog4j.configurationFile=…` for the downloaded logging config.
    pub logging_arg: Option<String>,
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// Build the effective version json for an instance, honouring `inheritsFrom`.
pub async fn resolve(
    http: &reqwest::Client,
    paths: &AppPaths,
    game_version: &str,
    loader: LoaderKind,
    loader_version: Option<&str>,
) -> Result<VersionJson> {
    // Always start from the vanilla version.
    let manifest = manifest::fetch_manifest(http, paths, false).await?;
    let summary = find_version(&manifest, game_version)?;
    let vanilla = manifest::fetch_version_json(http, paths, summary).await?;

    let Some(lv) = loader_version else {
        return Ok(vanilla);
    };
    if loader == LoaderKind::Vanilla {
        return Ok(vanilla);
    }

    let id = loader::profile_id(loader, game_version, lv);
    let profile_file = loader::profile_file(paths, &id);
    let profile: VersionJson = read_json(&profile_file)?.ok_or_else(|| {
        LauncherError::msg(format!(
            "The {loader:?} profile isn't installed yet. Install the loader for this instance first."
        ))
    })?;

    // Forge profiles point at the vanilla version they were built for, which
    // may differ from the one we resolved (e.g. "1.20.1"), so load the parent
    // from `inheritsFrom` when present.
    let parent = match profile.inherited_from.as_deref() {
        Some(parent_id) if parent_id != game_version => {
            match find_version(&manifest, parent_id) {
                Ok(parent_summary) => {
                    manifest::fetch_version_json(http, paths, parent_summary).await?
                }
                Err(_) => vanilla,
            }
        }
        _ => vanilla,
    };

    Ok(merge_versions(parent, profile))
}

// ---------------------------------------------------------------------------
// File installation
// ---------------------------------------------------------------------------

/// Download the client jar, libraries, assets and natives for a version.
pub async fn ensure_files(
    http: &reqwest::Client,
    paths: &AppPaths,
    version: &VersionJson,
    base_version: &str,
    concurrency: usize,
    sink: ProgressSink,
) -> Result<InstalledVersion> {
    let features = Features::default();

    // ---- client jar -------------------------------------------------------
    let client_jar = paths
        .versions_dir()
        .join(base_version)
        .join(format!("{base_version}.jar"));

    if let Some(client) = version.downloads.get("client") {
        sink(Progress::new("download", "Downloading the Minecraft client…"));
        let mut spec = DownloadSpec::new(client.url.clone(), &client_jar)
            .with_size(client.size)
            .with_label("client.jar");
        if !client.sha1.is_empty() {
            spec = spec.with_sha1(client.sha1.clone());
        }
        download_many(
            http.clone(),
            vec![spec],
            1,
            "download",
            sink.clone(),
        )
        .await?;
    }

    // ---- libraries --------------------------------------------------------
    let natives_dir = paths.cache_dir().join("natives").join(&version.id);
    std::fs::create_dir_all(&natives_dir)?;

    let mut specs: Vec<DownloadSpec> = vec![];
    let mut classpath: Vec<PathBuf> = vec![];
    let mut native_jars: Vec<(PathBuf, Vec<String>)> = vec![];
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for library in &version.libraries {
        if !rules_allow(&library.rules, &features) {
            continue;
        }

        // Old-style natives: a classifier jar that must be extracted.
        if let Some(natives) = &library.natives {
            if let Some(classifier) = natives.get(current_os_name()) {
                let classifier = classifier.replace("${arch}", arch_bits());
                let entry = library
                    .downloads
                    .as_ref()
                    .and_then(|d| d.classifiers.as_ref())
                    .and_then(|c| c.get(&classifier))
                    .cloned();

                let (url, sha1, size) = match entry {
                    Some(e) => (e.url, e.sha1, e.size),
                    None => {
                        let (rel, _) = maven_path(&format!(
                            "{}:{}",
                            library.name,
                            classifier
                        ));
                        let base = library
                            .url
                            .clone()
                            .unwrap_or_else(|| "https://libraries.minecraft.net/".into());
                        (format!("{}{}", ensure_trailing_slash(&base), rel), String::new(), 0)
                    }
                };

                let dest = paths.libraries_dir().join(maven_relative(&library.name, &classifier));
                if seen.insert(dest.to_string_lossy().to_string()) {
                    let mut spec = DownloadSpec::new(url, &dest).with_label("natives");
                    if !sha1.is_empty() {
                        spec = spec.with_sha1(sha1).with_size(size);
                    }
                    specs.push(spec);
                }
                let exclude = library
                    .extract
                    .as_ref()
                    .map(|e| e.exclude.clone())
                    .unwrap_or_default();
                native_jars.push((dest, exclude));
            }
            continue;
        }

        // Regular library dependency.
        let entry = library
            .downloads
            .as_ref()
            .and_then(|d| d.artifact.as_ref())
            .cloned();

        let (rel, _) = maven_path(&library.name);
        let dest = paths.libraries_dir().join(&rel);
        let (url, sha1, size) = match entry {
            Some(e) => (e.url, e.sha1, e.size),
            None => {
                let base = library
                    .url
                    .clone()
                    .unwrap_or_else(|| "https://libraries.minecraft.net/".into());
                (format!("{}{rel}", ensure_trailing_slash(&base)), String::new(), 0)
            }
        };

        if seen.insert(dest.to_string_lossy().to_string()) {
            let mut spec = DownloadSpec::new(url, &dest).with_label("library");
            if !sha1.is_empty() {
                spec = spec.with_sha1(sha1).with_size(size);
            }
            specs.push(spec);
        }
        classpath.push(dest);
    }

    // ---- assets -----------------------------------------------------------
    if let Some(index) = &version.asset_index {
        let index_file = paths.assets_dir().join("indexes").join(format!("{}.json", index.id));
        let mut spec = DownloadSpec::new(index.url.clone(), &index_file)
            .with_size(index.size)
            .with_label("asset index");
        if !index.sha1.is_empty() {
            spec = spec.with_sha1(index.sha1.clone());
        }
        download_many(http.clone(), vec![spec], 1, "download", sink.clone())
            .await?;

        let index_data: AssetIndexObjects = read_json(&index_file)?.unwrap_or_default();
        let objects_dir = paths.assets_dir().join("objects");
        let mut asset_specs = Vec::with_capacity(index_data.objects.len());
        for (name, object) in &index_data.objects {
            if object.hash.len() < 2 {
                continue;
            }
            let prefix = &object.hash[..2];
            let dest = objects_dir.join(prefix).join(&object.hash);
            asset_specs.push(
                DownloadSpec::new(
                    format!("https://resources.download.minecraft.net/{prefix}/{}", object.hash),
                    dest,
                )
                .with_sha1(object.hash.clone())
                .with_size(object.size)
                .with_label(short_name(name)),
            );
        }
        download_many(
            http.clone(),
            asset_specs,
            concurrency.max(4),
            "download",
            sink.clone(),
        )
        .await?;
    }

    // ---- logging config (Log4Shell mitigation) ----------------------------
    let mut logging_arg = None;
    if let Some(client) = version.logging.as_ref().and_then(|l| l.client.as_ref()) {
        let dest = paths.assets_dir().join("log_configs").join(&client.file.id);
        let mut spec = DownloadSpec::new(client.file.url.clone(), &dest).with_label("log config");
        if !client.file.sha1.is_empty() {
            spec = spec.with_sha1(client.file.sha1.clone()).with_size(client.file.size);
        }
        specs.push(spec);
        logging_arg = Some(client.argument.replace("${path}", &dest.to_string_lossy()));
    }

    // ---- bulk library download -------------------------------------------
    if !specs.is_empty() {
        download_many(http.clone(), specs, concurrency, "download", sink.clone()).await?;
    }

    // ---- natives extraction ----------------------------------------------
    if !native_jars.is_empty() {
        sink(Progress::new("extract", "Extracting native libraries…"));
        let dir = natives_dir.clone();
        let jars = native_jars.clone();
        tokio::task::spawn_blocking(move || extract_natives(&jars, &dir))
            .await
            .map_err(|e| LauncherError::msg(format!("native extraction failed: {e}")))??;
    }

    // The client jar comes first on the classpath, like the vanilla launcher.
    let mut full_classpath = vec![client_jar];
    full_classpath.extend(classpath);

    Ok(InstalledVersion {
        java_major: version
            .java_version
            .as_ref()
            .map(|j| j.major_version)
            .unwrap_or(8),
        main_class: version.main_class.clone(),
        asset_index: version
            .asset_index
            .as_ref()
            .map(|a| a.id.clone())
            .unwrap_or_else(|| "legacy".into()),
        version: version.clone(),
        classpath: full_classpath,
        natives_dir,
        logging_arg,
    })
}

#[derive(serde::Deserialize, Default)]
struct AssetIndexObjects {
    #[serde(default)]
    objects: HashMap<String, AssetObject>,
}

#[derive(serde::Deserialize)]
struct AssetObject {
    hash: String,
    #[serde(default)]
    size: u64,
}

fn short_name(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_string()
}

fn arch_bits() -> &'static str {
    if cfg!(target_pointer_width = "64") {
        "64"
    } else {
        "32"
    }
}

fn ensure_trailing_slash(url: &str) -> String {
    if url.ends_with('/') {
        url.to_string()
    } else {
        format!("{url}/")
    }
}

/// Maven on-disk path for a library including an extra classifier.
fn maven_relative(name: &str, classifier: &str) -> String {
    let (base, ext) = maven_path(name);
    if classifier.is_empty() {
        return base;
    }
    let file_name = base.rsplit('/').next().unwrap_or(&base);
    let stem = file_name.strip_suffix(&format!(".{ext}")).unwrap_or(file_name);
    base.replace(file_name, &format!("{stem}-{classifier}.{ext}"))
}

/// Extract native libraries out of their classifier jars, honouring `extract.exclude`.
fn extract_natives(jars: &[(PathBuf, Vec<String>)], dest: &Path) -> Result<()> {
    for (jar, exclude) in jars {
        if !jar.exists() {
            continue;
        }
        let file = std::fs::File::open(jar)?;
        let mut archive = zip::ZipArchive::new(file)?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let raw_name = entry.name().to_string();

            // Never extract signatures, manifests or anything explicitly excluded.
            if raw_name.starts_with("META-INF/") {
                continue;
            }
            if exclude.iter().any(|e| raw_name.starts_with(e)) {
                continue;
            }
            if entry.is_dir() {
                continue;
            }

            let file_name = Path::new(&raw_name)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| raw_name.clone());
            let out_path = dest.join(&file_name);

            let mut out = std::fs::File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out)?;

            // Preserve the executable bit for .so/.dylib on unix.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if raw_name.ends_with(".so") || raw_name.ends_with(".dylib") {
                    let _ = std::fs::set_permissions(
                        &out_path,
                        std::fs::Permissions::from_mode(0o755),
                    );
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Launch command
// ---------------------------------------------------------------------------

/// Build the full `java` argument vector for a launch.
pub fn build_command(ctx: &LaunchContext) -> Result<(String, Vec<String>)> {
    let features = Features {
        has_custom_resolution: ctx.width > 0 && ctx.height > 0,
        has_quick_plays_support: !matches!(ctx.quick_play, QuickPlay::None),
        is_quick_play_singleplayer: matches!(
            ctx.quick_play,
            QuickPlay::Singleplayer { .. } | QuickPlay::SingleplayerContinue
        ),
        is_quick_play_multiplayer: matches!(ctx.quick_play, QuickPlay::Multiplayer { .. }),
        ..Default::default()
    };

    let classpath = ctx
        .classpath
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(if cfg!(windows) { ";" } else { ":" });

    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert("auth_player_name", ctx.username.clone());
    vars.insert("version_name", ctx.version.id.clone());
    vars.insert("game_directory", ctx.game_dir.to_string_lossy().to_string());
    vars.insert("assets_root", ctx.assets_dir.to_string_lossy().to_string());
    vars.insert("assets_index_name", ctx.asset_index.clone());
    vars.insert("auth_uuid", ctx.uuid.replace('-', ""));
    vars.insert("auth_access_token", ctx.access_token.clone());
    vars.insert("auth_session", format!("token:{}", ctx.access_token));
    vars.insert("user_type", ctx.user_type.clone());
    vars.insert("version_type", "MineArcade".into());
    vars.insert("natives_directory", ctx.natives_dir.to_string_lossy().to_string());
    vars.insert("launcher_name", "MineArcade".into());
    vars.insert("launcher_version", env!("CARGO_PKG_VERSION").into());
    vars.insert("classpath", classpath.clone());
    vars.insert("resolution_width", ctx.width.to_string());
    vars.insert("resolution_height", ctx.height.to_string());
    vars.insert("user_properties", "{}".into());
    vars.insert("auth_xuid", "0".into());
    vars.insert("clientid", "minearcade".into());
    // Quick Play placeholders (1.20+).
    match &ctx.quick_play {
        QuickPlay::Multiplayer { address } => {
            vars.insert("quickPlayMultiplayer", address.clone());
        }
        QuickPlay::Singleplayer { world } => {
            vars.insert("quickPlaySingleplayer", world.clone());
        }
        QuickPlay::SingleplayerContinue => {
            vars.insert("quickPlaySingleplayer", "".into());
        }
        QuickPlay::None => {}
    }

    let mut args: Vec<String> = vec![];

    // ---- JVM arguments ----------------------------------------------------
    args.push(format!("-Xms{}M", ctx.memory_min.max(256)));
    args.push(format!("-Xmx{}M", ctx.memory_max.max(512)));
    // Log4Shell: the flag covers log4j 2.10+, Mojang's patched config covers the rest.
    args.push("-Dlog4j2.formatMsgNoLookups=true".into());
    if let Some(logging) = &ctx.logging_arg {
        args.push(logging.clone());
    }

    let jvm_args = ctx
        .version
        .arguments
        .as_ref()
        .map(|a| a.jvm.clone())
        .unwrap_or_default();

    if jvm_args.is_empty() {
        // Very old versions don't ship JVM args; emulate the vanilla defaults.
        args.push(format!(
            "-Djava.library.path={}",
            ctx.natives_dir.to_string_lossy()
        ));
        args.push("-cp".into());
        args.push(classpath.clone());
    } else {
        expand_arguments(&jvm_args, &vars, &features, &mut args);
    }

    for arg in &ctx.extra_jvm_args {
        if !arg.trim().is_empty() {
            args.push(arg.clone());
        }
    }

    args.push(ctx.version.main_class.clone());

    // ---- Game arguments ---------------------------------------------------
    match &ctx.version.arguments {
        Some(Arguments { game, .. }) if !game.is_empty() => {
            expand_arguments(game, &vars, &features, &mut args);
        }
        _ => {
            // Pre-1.13 style single string.
            if let Some(legacy) = &ctx.version.minecraft_arguments {
                let substituted = substitute(legacy, &vars);
                args.extend(substituted.split_whitespace().map(|s| s.to_string()));
            }
        }
    }

    for arg in &ctx.extra_game_args {
        if !arg.trim().is_empty() {
            args.push(substitute(arg, &vars));
        }
    }

    // Quick Play only exists from 1.20; older versions join a server through
    // the legacy --server/--port arguments instead.
    if let QuickPlay::Multiplayer { address } = &ctx.quick_play {
        if !args.iter().any(|arg| arg == "--quickPlayMultiplayer") {
            let (host, port) = split_server_address(address);
            args.extend(["--server".to_string(), host, "--port".to_string(), port]);
        }
    }

    if ctx.fullscreen {
        args.push("--fullscreen".into());
    }

    Ok(("java".to_string(), args))
}

/// Split `host[:port]` for the legacy `--server`/`--port` arguments.
fn split_server_address(address: &str) -> (String, String) {
    let address = address.trim();
    match address.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && port.parse::<u16>().is_ok() => {
            (host.to_string(), port.to_string())
        }
        _ => (address.to_string(), "25565".to_string()),
    }
}

#[cfg(test)]
mod server_address_tests {
    use super::split_server_address;

    #[test]
    fn splits_host_and_port() {
        assert_eq!(
            split_server_address("play.minearcade.org"),
            ("play.minearcade.org".into(), "25565".into())
        );
        assert_eq!(
            split_server_address("play.minearcade.org:25570"),
            ("play.minearcade.org".into(), "25570".into())
        );
        assert_eq!(
            split_server_address("host:notaport"),
            ("host:notaport".into(), "25565".into())
        );
    }
}

/// Expand a Mojang argument list, skipping rules that don't match and
/// flattening array values (e.g. `["-cp", "${classpath}"]`).
fn expand_arguments(
    list: &[serde_json::Value],
    vars: &HashMap<&str, String>,
    features: &Features,
    out: &mut Vec<String>,
) {
    for item in list {
        // Plain string form.
        if let Some(value) = item.as_str() {
            out.push(substitute(value, vars));
            continue;
        }

        let Some(object) = item.as_object() else {
            continue;
        };

        let rules: Vec<manifest::Rule> = object
            .get("rules")
            .and_then(|r| serde_json::from_value(r.clone()).ok())
            .unwrap_or_default();
        if !rules_allow(&rules, features) {
            continue;
        }

        match object.get("value") {
            Some(serde_json::Value::String(value)) => out.push(substitute(value, vars)),
            Some(serde_json::Value::Array(values)) => {
                for value in values {
                    if let Some(value) = value.as_str() {
                        out.push(substitute(value, vars));
                    }
                }
            }
            _ => {}
        }
    }
}

/// Replace `${placeholder}` tokens with their values. Unknown tokens become
/// empty strings so the game doesn't see literal `${...}`.
fn substitute(input: &str, vars: &HashMap<&str, String>) -> String {
    if !input.contains("${") {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end) = input[i + 2..].find('}') {
                let key = &input[i + 2..i + 2 + end];
                out.push_str(vars.get(key).map(|s| s.as_str()).unwrap_or(""));
                i = i + 2 + end + 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Cheap, synchronous check that the instance's version profile is available.
///
/// Used to render "installed" badges without touching the network.
pub fn resolve_cached(paths: &AppPaths, instance: &crate::core::instance::Instance) -> Option<()> {
    use crate::core::instance::LoaderKind;

    match instance.loader {
        LoaderKind::Vanilla => manifest::cached_version_file(paths, &instance.game_version)
            .exists()
            .then_some(()),
        kind => {
            let loader_version = instance.loader_version.as_deref()?;
            let id = loader::profile_id(kind, &instance.game_version, loader_version);
            loader::profile_file(paths, &id).exists().then_some(())
        }
    }
}

/// Grab the vanilla version id used as the base for a loader profile.
pub fn base_version_of(version: &VersionJson, fallback: &str) -> String {
    version
        .inherited_from
        .clone()
        .unwrap_or_else(|| fallback.to_string())
}

/// Read a cached version json for the given profile id.
pub fn read_cached_profile(paths: &AppPaths, id: &str) -> Result<Option<VersionJson>> {
    read_json(&manifest::cached_version_file(paths, id))
}

/// Persist a version json into the metadata cache.
pub fn write_cached_profile(paths: &AppPaths, id: &str, version: &VersionJson) -> Result<()> {
    write_json(&manifest::cached_version_file(paths, id), version)
}
