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
    let mut manifest = manifest::fetch_manifest(http, paths, false).await?;
    if find_version(&manifest, game_version).is_err() {
        // The cached manifest may predate this version; refresh it once. When
        // offline, fall through to the original "not found".
        if let Ok(fresh) = manifest::fetch_manifest(http, paths, true).await {
            manifest = fresh;
        }
    }
    let summary = find_version(&manifest, game_version)?;
    let vanilla = manifest::fetch_version_json(http, paths, summary).await?;

    let Some(lv) = loader_version else {
        return Ok(vanilla);
    };
    if loader == LoaderKind::Vanilla {
        return Ok(vanilla);
    }

    loader::validate_version("Minecraft version", game_version)?;
    loader::validate_version("Loader version", lv)?;
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

/// The game jar to put on the classpath for `version`.
///
/// Forge and NeoForge (1.17+) load the game themselves, from their own patched copy, and
/// tell their bootstrap to skip `${version_name}.jar`: the name the official launcher gives
/// the game jar of a loader version, a copy in the loader version's own folder. Under its
/// vanilla name (`1.20.1.jar`) it isn't skipped, the game loads twice and Java refuses to
/// start ("Modules minecraft and _1._20._1 export package net.minecraft.server ..."). So
/// those versions get that same copy; everything else uses the vanilla jar where it is.
fn loader_named_client_jar(paths: &AppPaths, version: &VersionJson, base_version: &str, vanilla: PathBuf) -> Result<PathBuf> {
    let skips_named_jar = serde_json::to_string(&version.arguments)
        .map(|args| args.contains("${version_name}.jar"))
        .unwrap_or(false);
    if !skips_named_jar || version.id == base_version || !vanilla.is_file() {
        return Ok(vanilla);
    }
    let named = paths.versions_dir().join(&version.id).join(format!("{}.jar", version.id));
    let size = |path: &Path| std::fs::metadata(path).map(|m| m.len()).ok();
    if size(&named) != size(&vanilla) {
        if let Some(dir) = named.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temp = named.with_extension("jar.tmp");
        std::fs::copy(&vanilla, &temp)?;
        std::fs::rename(&temp, &named)?;
    }
    Ok(named)
}

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
        }

        // Regular library dependency. A library may carry both natives and a
        // main artifact; natives-only libraries have no jar of their own.
        let entry = library
            .downloads
            .as_ref()
            .and_then(|d| d.artifact.as_ref())
            .cloned();
        if library.natives.is_some() && entry.is_none() {
            continue;
        }

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

        if url.is_empty() {
            // Generated by the Forge/NeoForge installer (e.g. `…:client`):
            // there is nothing to download, the installer must have made it.
            if !dest.is_file() {
                return Err(LauncherError::msg(format!(
                    "{} is missing. Reinstall the mod loader for this instance.",
                    library.name
                )));
            }
        } else if seen.insert(dest.to_string_lossy().to_string()) {
            let mut spec = DownloadSpec::new(url, &dest).with_label("library");
            if !sha1.is_empty() {
                spec = spec.with_sha1(sha1).with_size(size);
            }
            specs.push(spec);
        }
        // Each jar once: the same jar twice on the classpath crashes Forge-family loaders.
        if !classpath.contains(&dest) {
            classpath.push(dest);
        }
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
                .with_label(short_name(name))
                // Named by their own hash: a file of the right size is the right file.
                .with_quick_verify(),
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
    let client_jar = loader_named_client_jar(paths, version, base_version, client_jar)?;
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
    /// 1.6–1.7.2 (`legacy`): objects must also exist under their real names.
    #[serde(default, rename = "virtual")]
    is_virtual: bool,
    /// Pre-1.6 (`pre-1.6`): objects must be copied into `<game dir>/resources`.
    #[serde(default)]
    map_to_resources: bool,
}

/// The `${game_assets}` directory for an asset index. Legacy indexes want
/// their objects laid out under their real names — in `assets/virtual/<index>`
/// or the instance's `resources` folder — so materialise that copy first.
fn game_assets_dir(assets_dir: &Path, index_id: &str, game_dir: &Path) -> Result<PathBuf> {
    let index_file = assets_dir.join("indexes").join(format!("{index_id}.json"));
    let index: AssetIndexObjects = read_json(&index_file)?.unwrap_or_default();
    let target = if index.map_to_resources {
        game_dir.join("resources")
    } else if index.is_virtual {
        assets_dir.join("virtual").join(index_id)
    } else {
        return Ok(assets_dir.to_path_buf());
    };

    for (name, object) in &index.objects {
        let relative = Path::new(name);
        let safe = relative
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
        if object.hash.len() < 2 || !safe {
            continue;
        }
        let dest = target.join(relative);
        if std::fs::metadata(&dest).is_ok_and(|m| m.len() == object.size) {
            continue;
        }
        let source = assets_dir.join("objects").join(&object.hash[..2]).join(&object.hash);
        if !source.is_file() {
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&source, &dest)?;
    }
    Ok(target)
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

            // The natives folder is shared per version: a running instance
            // keeps these loaded (and locked on Windows), so leave an
            // identical copy alone instead of rewriting it.
            if std::fs::metadata(&out_path).is_ok_and(|m| m.len() == entry.size()) {
                continue;
            }

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

    let separator = if cfg!(windows) { ";" } else { ":" };
    let classpath = ctx
        .classpath
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(separator);

    // Forge/NeoForge JVM args reference the libraries folder, which sits next
    // to `assets` in our meta directory.
    let library_directory = ctx
        .assets_dir
        .parent()
        .map(|meta| meta.join("libraries"))
        .unwrap_or_default();
    let game_assets = game_assets_dir(&ctx.assets_dir, &ctx.asset_index, &ctx.game_dir)?;

    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert("auth_player_name", ctx.username.clone());
    vars.insert("version_name", ctx.version.id.clone());
    vars.insert("game_directory", ctx.game_dir.to_string_lossy().to_string());
    vars.insert("assets_root", ctx.assets_dir.to_string_lossy().to_string());
    vars.insert("assets_index_name", ctx.asset_index.clone());
    vars.insert("game_assets", game_assets.to_string_lossy().to_string());
    vars.insert("library_directory", library_directory.to_string_lossy().to_string());
    vars.insert("classpath_separator", separator.into());
    vars.insert("auth_uuid", ctx.uuid.replace('-', ""));
    vars.insert("auth_access_token", ctx.access_token.clone());
    vars.insert("auth_session", format!("token:{}", ctx.access_token));
    vars.insert("user_type", ctx.user_type.clone());
    vars.insert("version_type", "Orbit Launcher".into());
    vars.insert("natives_directory", ctx.natives_dir.to_string_lossy().to_string());
    vars.insert("launcher_name", "Orbit Launcher".into());
    vars.insert("launcher_version", env!("CARGO_PKG_VERSION").into());
    vars.insert("classpath", classpath.clone());
    vars.insert("resolution_width", ctx.width.to_string());
    vars.insert("resolution_height", ctx.height.to_string());
    vars.insert("user_properties", "{}".into());
    vars.insert("auth_xuid", "0".into());
    vars.insert("clientid", "orbit-launcher".into());
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
            // Pre-1.13 style single string. Split the template, not the
            // result, so paths containing spaces stay one argument.
            if let Some(legacy) = &ctx.version.minecraft_arguments {
                args.extend(legacy.split_whitespace().map(|token| substitute(token, &vars)));
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
            loader::validate_version("Minecraft version", &instance.game_version).ok()?;
            loader::validate_version("Loader version", loader_version).ok()?;
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

#[cfg(test)]
mod command_tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbit-launcher-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn context(version: VersionJson, root: &Path) -> LaunchContext {
        LaunchContext {
            version,
            base_version: "1.0".into(),
            game_dir: root.join("my instance"),
            assets_dir: root.join("meta").join("assets"),
            asset_index: "none".into(),
            natives_dir: root.join("natives"),
            classpath: vec![root.join("a.jar"), root.join("b.jar")],
            username: "Steve".into(),
            uuid: "uuid".into(),
            access_token: "token".into(),
            user_type: "msa".into(),
            width: 0,
            height: 0,
            fullscreen: false,
            memory_min: 512,
            memory_max: 1024,
            extra_jvm_args: vec![],
            extra_game_args: vec![],
            quick_play: QuickPlay::None,
            logging_arg: None,
        }
    }

    #[test]
    fn legacy_arguments_keep_paths_with_spaces_whole() {
        let root = Path::new("/data/orbit");
        let version = VersionJson {
            main_class: "net.minecraft.client.main.Main".into(),
            minecraft_arguments: Some("--username ${auth_player_name} --gameDir ${game_directory}".into()),
            ..Default::default()
        };
        let (_, args) = build_command(&context(version, root)).unwrap();
        let at = args.iter().position(|a| a == "--gameDir").unwrap();
        assert_eq!(args[at + 1], root.join("my instance").to_string_lossy());
    }

    #[test]
    fn defines_forge_placeholders() {
        let root = Path::new("/data/orbit");
        let version = VersionJson {
            main_class: "cpw.mods.bootstraplauncher.BootstrapLauncher".into(),
            arguments: Some(Arguments {
                jvm: vec![
                    "-DlibraryDirectory=${library_directory}".into(),
                    "-p".into(),
                    "${library_directory}/x.jar${classpath_separator}${library_directory}/y.jar".into(),
                ],
                game: vec![],
            }),
            ..Default::default()
        };
        let (_, args) = build_command(&context(version, root)).unwrap();
        let libraries = root.join("meta").join("libraries").to_string_lossy().to_string();
        let sep = if cfg!(windows) { ";" } else { ":" };
        assert!(args.contains(&format!("-DlibraryDirectory={libraries}")));
        assert!(args.contains(&format!("{libraries}/x.jar{sep}{libraries}/y.jar")));
    }

    #[test]
    fn virtual_asset_index_is_materialised() {
        let root = temp_dir("virtual-assets");
        let assets = root.join("assets");
        std::fs::create_dir_all(assets.join("indexes")).unwrap();
        std::fs::create_dir_all(assets.join("objects").join("ab")).unwrap();
        std::fs::write(assets.join("objects").join("ab").join("abcdef"), b"hello").unwrap();
        std::fs::write(
            assets.join("indexes").join("legacy.json"),
            r#"{"virtual":true,"objects":{"sounds/a.ogg":{"hash":"abcdef","size":5},"../evil":{"hash":"abcdef","size":5}}}"#,
        )
        .unwrap();

        let dir = game_assets_dir(&assets, "legacy", &root.join("game")).unwrap();
        assert_eq!(dir, assets.join("virtual").join("legacy"));
        assert_eq!(std::fs::read(dir.join("sounds").join("a.ogg")).unwrap(), b"hello");
        assert!(!assets.join("virtual").join("evil").exists());

        std::fs::write(
            assets.join("indexes").join("pre-1.6.json"),
            r#"{"map_to_resources":true,"objects":{"sound/b.ogg":{"hash":"abcdef","size":5}}}"#,
        )
        .unwrap();
        let dir = game_assets_dir(&assets, "pre-1.6", &root.join("game")).unwrap();
        assert_eq!(dir, root.join("game").join("resources"));
        assert!(dir.join("sound").join("b.ogg").is_file());

        // Modern indexes use the shared assets folder untouched.
        assert_eq!(game_assets_dir(&assets, "17", &root.join("game")).unwrap(), assets);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn natives_already_extracted_are_left_alone() {
        use std::io::Write;
        let root = temp_dir("natives");
        let jar = root.join("natives.jar");
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&jar).unwrap());
            zip.start_file("lwjgl.dll", zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(b"native").unwrap();
            zip.finish().unwrap();
        }
        let out = root.join("out");
        std::fs::create_dir_all(&out).unwrap();
        extract_natives(&[(jar.clone(), vec![])], &out).unwrap();
        assert_eq!(std::fs::read(out.join("lwjgl.dll")).unwrap(), b"native");

        // Same size: treated as the same file (it may be locked by a running game).
        std::fs::write(out.join("lwjgl.dll"), b"locked").unwrap();
        extract_natives(&[(jar, vec![])], &out).unwrap();
        assert_eq!(std::fs::read(out.join("lwjgl.dll")).unwrap(), b"locked");
        let _ = std::fs::remove_dir_all(&root);
    }
}
