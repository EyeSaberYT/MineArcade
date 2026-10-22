//! Prepares an instance (account, version files, Java, natives, assets) and starts the game.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{bail, Context, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::oneshot;

use crate::config::{self, Loader};
use crate::download::{download_all, Download};
use crate::progress::Progress;
use crate::state::AppState;
use crate::versions::{self, Features, NativeJar};
use crate::{auth, fabric, java};

const LAUNCHER_NAME: &str = "MineArcade";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LogEvent {
    instance_id: String,
    level: String,
    line: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartEvent {
    instance_id: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExitEvent {
    instance_id: String,
    code: Option<i32>,
}

/// Removes the instance from the "launching" set however `launch` returns.
struct LaunchGuard<'a> {
    state: &'a AppState,
    id: String,
}

impl Drop for LaunchGuard<'_> {
    fn drop(&mut self) {
        self.state.launching.lock().unwrap().remove(&self.id);
    }
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn emit_log(app: &AppHandle, instance_id: &str, level: &str, line: impl Into<String>) {
    let _ = app.emit(
        "game-log",
        LogEvent {
            instance_id: instance_id.to_string(),
            level: level.to_string(),
            line: line.into(),
        },
    );
}

pub async fn launch(app: &AppHandle, state: &AppState, instance_id: &str, server_override: Option<String>) -> Result<()> {
    if state.running.lock().unwrap().contains_key(instance_id) {
        bail!("This instance is already running.");
    }
    if !state.launching.lock().unwrap().insert(instance_id.to_string()) {
        bail!("This instance is already starting.");
    }
    let _guard = LaunchGuard {
        state,
        id: instance_id.to_string(),
    };

    let progress = Progress::new(app.clone(), instance_id);
    let settings = config::load_settings(&state.paths);
    let instance = config::load_instances(&state.paths)
        .into_iter()
        .find(|i| i.id == instance_id)
        .context("This instance no longer exists.")?;

    // 1. Account
    progress.stage("auth", "Checking your Microsoft account");
    let account = auth::active_account(state, &settings.effective_client_id()).await?;

    // 2. Version JSON (Fabric profiles inherit from the vanilla version)
    progress.stage("version", format!("Preparing Minecraft {}", instance.mc_version));
    let top = match instance.loader {
        Loader::Vanilla => versions::ensure_vanilla_json(state, &instance.mc_version).await?,
        Loader::Fabric => {
            let loader_version = match instance.loader_version.as_deref().filter(|v| !v.is_empty()) {
                Some(v) => v.to_string(),
                None => fabric::latest_loader(&state.http, &instance.mc_version).await?,
            };
            fabric::ensure_profile(state, &instance.mc_version, &loader_version).await?
        }
    };
    let resolved = versions::resolve(state, top).await?;
    let version = &resolved.json;

    let server = server_override
        .or_else(|| instance.auto_join.clone())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let quick_play = server.is_some() && versions::supports_quick_play(version);
    let features = Features {
        quick_play_multiplayer: quick_play,
        ..Features::default()
    };

    // 3. Java
    let java = match instance.java_path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(custom) => PathBuf::from(custom),
        None => {
            let component = version
                .java_version
                .as_ref()
                .map(|j| j.component.as_str())
                .unwrap_or("jre-legacy");
            java::ensure_runtime(state, component, &progress).await?
        }
    };

    // 4. Game files: client jar, libraries, assets, logging config
    progress.stage("download", "Checking game files");
    let client = version
        .downloads
        .as_ref()
        .and_then(|d| d.client.as_ref())
        .context("This version has no client download.")?;
    let client_jar = state.paths.version_jar(&resolved.jar_id);
    let mut downloads = vec![Download {
        url: client.url.clone(),
        path: client_jar.clone(),
        sha1: client.sha1.clone(),
        size: client.size,
        executable: false,
    }];

    let libraries = versions::plan_libraries(&state.paths, &version.libraries, &features);
    downloads.extend(libraries.downloads.iter().cloned());

    let asset_ref = version
        .asset_index
        .as_ref()
        .context("This version has no asset index.")?;
    let asset_index = versions::ensure_asset_index(state, asset_ref).await?;
    downloads.extend(versions::asset_downloads(&state.paths, &asset_index));

    // Mojang's logging config also carries the Log4Shell mitigation for older versions.
    let mut logging_arg = None;
    if let Some(log) = version.logging.as_ref().and_then(|l| l.client.as_ref()) {
        let path = state.paths.assets().join("log_configs").join(&log.file.id);
        downloads.push(Download {
            url: log.file.url.clone(),
            path: path.clone(),
            sha1: log.file.sha1.clone(),
            size: log.file.size,
            executable: false,
        });
        logging_arg = Some(log.argument.replace("${path}", &path_string(&path)));
    }

    progress.stage("download", "Downloading game files");
    download_all(&state.http, downloads, |done, total| progress.update(done, total)).await?;

    // 5. Natives and legacy asset layouts
    progress.stage("natives", "Unpacking native libraries");
    let natives_dir = state.paths.natives_dir(&instance.id);
    let game_dir = state.paths.instance_dir(&instance.id);
    let game_assets = {
        let natives = libraries.natives.clone();
        let natives_dir = natives_dir.clone();
        let game_dir = game_dir.clone();
        let paths = state.paths.clone();
        let index_id = asset_ref.id.clone();
        tokio::task::spawn_blocking(move || -> Result<PathBuf> {
            extract_natives(&natives_dir, &natives)?;
            std::fs::create_dir_all(&game_dir)?;
            if asset_index.map_to_resources {
                let target = game_dir.join("resources");
                versions::copy_assets(&paths, &asset_index, &target)?;
                Ok(target)
            } else if asset_index.is_virtual {
                let target = paths.assets().join("virtual").join(&index_id);
                versions::copy_assets(&paths, &asset_index, &target)?;
                Ok(target)
            } else {
                Ok(paths.assets())
            }
        })
        .await??
    };

    // 6. Arguments
    let separator = if cfg!(windows) { ";" } else { ":" };
    let mut classpath: Vec<String> = libraries.classpath.iter().map(|p| path_string(p)).collect();
    classpath.push(path_string(&client_jar));

    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert("auth_player_name", account.username.clone());
    vars.insert("version_name", version.id.clone());
    vars.insert("game_directory", path_string(&game_dir));
    vars.insert("assets_root", path_string(&state.paths.assets()));
    vars.insert("game_assets", path_string(&game_assets));
    vars.insert("assets_index_name", asset_ref.id.clone());
    vars.insert("auth_uuid", account.uuid.clone());
    vars.insert("auth_access_token", account.mc_access_token.clone());
    vars.insert("auth_session", format!("token:{}:{}", account.mc_access_token, account.uuid));
    vars.insert("auth_xuid", account.xuid.clone());
    vars.insert("clientid", settings.effective_client_id());
    vars.insert("user_type", "msa".into());
    vars.insert("user_properties", "{}".into());
    vars.insert("version_type", version.kind.clone().unwrap_or_else(|| "release".into()));
    vars.insert("natives_directory", path_string(&natives_dir));
    vars.insert("launcher_name", LAUNCHER_NAME.into());
    vars.insert("launcher_version", env!("CARGO_PKG_VERSION").into());
    vars.insert("classpath", classpath.join(separator));
    vars.insert("classpath_separator", separator.into());
    vars.insert("library_directory", path_string(&state.paths.libraries()));
    vars.insert("resolution_width", "854".into());
    vars.insert("resolution_height", "480".into());
    if let Some(address) = &server {
        vars.insert("quickPlayMultiplayer", address.clone());
    }

    let memory = instance.memory_mb.max(512);
    let mut jvm = vec![
        format!("-Xmx{memory}M"),
        format!("-Xms{}M", memory.min(1024)),
        "-Dlog4j2.formatMsgNoLookups=true".to_string(),
    ];
    jvm.extend(instance.jvm_args.split_whitespace().map(str::to_string));
    if let Some(arg) = logging_arg {
        jvm.push(arg);
    }

    let mut game: Vec<String> = version
        .minecraft_arguments
        .as_deref()
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let mut version_jvm = Vec::new();
    if let Some(arguments) = &version.arguments {
        version_jvm = versions::collect_args(&arguments.jvm, &features);
        game.extend(versions::collect_args(&arguments.game, &features));
    }
    // Legacy versions (and Fabric on top of them) don't declare the classpath themselves.
    if !version_jvm.iter().any(|a| a.contains("${classpath}")) {
        if cfg!(target_os = "macos") {
            jvm.push("-XstartOnFirstThread".into());
        }
        if cfg!(windows) {
            jvm.push("-XX:HeapDumpPath=MojangTricksIntelDriversForPerformance_javaw.exe_minecraft.exe.heapdump".into());
        }
        jvm.push("-Djava.library.path=${natives_directory}".into());
        jvm.push("-cp".into());
        jvm.push("${classpath}".into());
    }
    jvm.extend(version_jvm);

    if let Some(address) = &server {
        if !quick_play {
            let (host, port) = split_server(address);
            game.extend(["--server".to_string(), host, "--port".to_string(), port]);
        }
    }

    let main_class = version
        .main_class
        .clone()
        .context("This version has no main class.")?;
    let mut args: Vec<String> = jvm.iter().map(|a| substitute(a, &vars)).collect();
    args.push(main_class);
    args.extend(game.iter().map(|a| substitute(a, &vars)));

    let token = account.mc_access_token.as_str();
    let shown: Vec<String> = args
        .iter()
        .map(|a| if !token.is_empty() && a.contains(token) { a.replace(token, "********") } else { a.clone() })
        .collect();
    emit_log(app, &instance.id, "LAUNCHER", format!("Java: {}", java.display()));
    emit_log(app, &instance.id, "LAUNCHER", format!("Arguments: {}", shown.join(" ")));
    if let Some(address) = &server {
        emit_log(app, &instance.id, "LAUNCHER", format!("Joining server {address}"));
    }

    // 7. Start the game
    progress.stage("launch", "Starting Minecraft");
    let mut command = tokio::process::Command::new(&java);
    command
        .args(&args)
        .current_dir(&game_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = command
        .spawn()
        .with_context(|| format!("Could not start Java ({})", java.display()))?;

    if let Some(stdout) = child.stdout.take() {
        spawn_log_reader(app.clone(), instance.id.clone(), stdout, false);
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_log_reader(app.clone(), instance.id.clone(), stderr, true);
    }

    let (kill_tx, mut kill_rx) = oneshot::channel::<()>();
    state.running.lock().unwrap().insert(instance.id.clone(), kill_tx);
    let _ = config::update_instance(&state.paths, &instance.id, |i| {
        i.last_played = Some(config::now_secs())
    });
    let _ = app.emit("game-started", StartEvent { instance_id: instance.id.clone() });

    let hide = settings.hide_on_launch;
    if hide {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.hide();
        }
    }

    let app = app.clone();
    let id = instance.id.clone();
    tauri::async_runtime::spawn(async move {
        let code = tokio::select! {
            status = child.wait() => status.ok().and_then(|s| s.code()),
            Ok(()) = &mut kill_rx => {
                let _ = child.kill().await;
                None
            }
        };
        app.state::<AppState>().running.lock().unwrap().remove(&id);
        if hide {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        let _ = app.emit("game-exit", ExitEvent { instance_id: id, code });
    });

    Ok(())
}

/// Replaces `${name}` placeholders; unknown placeholders are left untouched.
fn substitute(input: &str, vars: &HashMap<&str, String>) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                match vars.get(&after[..end]) {
                    Some(value) => out.push_str(value),
                    None => out.push_str(&rest[start..start + 2 + end + 1]),
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn split_server(address: &str) -> (String, String) {
    match address.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && port.parse::<u16>().is_ok() => {
            (host.to_string(), port.to_string())
        }
        _ => (address.to_string(), "25565".to_string()),
    }
}

fn extract_natives(dir: &Path, jars: &[NativeJar]) -> Result<()> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir)?;
    for jar in jars {
        let file = std::fs::File::open(&jar.path)
            .with_context(|| format!("could not open {}", jar.path.display()))?;
        let mut archive = zip::ZipArchive::new(file)
            .with_context(|| format!("{} is not a valid jar", jar.path.display()))?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let name = entry.name().to_string();
            if entry.is_dir()
                || name.starts_with("META-INF")
                || jar.exclude.iter().any(|prefix| name.starts_with(prefix.as_str()))
            {
                continue;
            }
            let Some(relative) = entry.enclosed_name() else { continue };
            let out = dir.join(relative);
            if out.exists() {
                continue;
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut target = std::fs::File::create(&out)?;
            std::io::copy(&mut entry, &mut target)?;
        }
    }
    Ok(())
}

fn spawn_log_reader<R>(app: AppHandle, instance_id: String, reader: R, is_stderr: bool)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tauri::async_runtime::spawn(async move {
        // Read raw bytes: Java may print non-UTF-8 text, and the pipe must keep draining
        // or the game would block once the buffer fills.
        let mut reader = BufReader::new(reader);
        let mut buffer = Vec::new();
        let mut parser = LogParser::default();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let text = String::from_utf8_lossy(&buffer);
                    let line = text.trim_end_matches(|c: char| c == '\r' || c == '\n');
                    if let Some((level, line)) = parser.feed(line, is_stderr) {
                        emit_log(&app, &instance_id, &level, line);
                    }
                }
            }
        }
    });
}

/// Mojang's log4j config prints XML events; turn them back into readable lines.
#[derive(Default)]
struct LogParser {
    pending: Option<String>,
}

impl LogParser {
    fn feed(&mut self, line: &str, is_stderr: bool) -> Option<(String, String)> {
        if let Some(pending) = self.pending.as_mut() {
            pending.push('\n');
            pending.push_str(line);
            if line.contains("</log4j:Event>") {
                let event = self.pending.take().unwrap_or_default();
                return Some(parse_event(&event));
            }
            return None;
        }
        if line.trim_start().starts_with("<log4j:Event") {
            if line.contains("</log4j:Event>") {
                return Some(parse_event(line));
            }
            self.pending = Some(line.to_string());
            return None;
        }
        Some((guess_level(line, is_stderr).to_string(), line.to_string()))
    }
}

fn guess_level(line: &str, is_stderr: bool) -> &'static str {
    if line.contains("/ERROR]") || line.contains("/FATAL]") || line.contains("Exception in thread") {
        "ERROR"
    } else if line.contains("/WARN]") {
        "WARN"
    } else if is_stderr {
        "WARN"
    } else {
        "INFO"
    }
}

fn attribute<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    let start = head.find(&key)? + key.len();
    let end = head[start..].find('"')? + start;
    Some(&head[start..end])
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = text.find(start)? + start.len();
    let to = text[from..].find(end)? + from;
    Some(&text[from..to])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_known_placeholders_only() {
        let vars = HashMap::from([("name", "Steve".to_string()), ("dir", "C:\\games".to_string())]);
        assert_eq!(substitute("--user ${name}", &vars), "--user Steve");
        assert_eq!(substitute("${dir}/x/${name}", &vars), "C:\\games/x/Steve");
        assert_eq!(substitute("${unknown} ${name}", &vars), "${unknown} Steve");
        assert_eq!(substitute("broken ${name", &vars), "broken ${name");
    }

    #[test]
    fn splits_server_addresses() {
        assert_eq!(split_server("play.example.org"), ("play.example.org".into(), "25565".into()));
        assert_eq!(split_server("play.example.org:25570"), ("play.example.org".into(), "25570".into()));
        assert_eq!(split_server("host:notaport"), ("host:notaport".into(), "25565".into()));
    }

    #[test]
    fn parses_log4j_xml_events() {
        let mut parser = LogParser::default();
        let lines = [
            r#"<log4j:Event logger="net.minecraft.client.Minecraft" timestamp="1" level="WARN" thread="Render thread">"#,
            "  <log4j:Message><![CDATA[Hello",
            "world]]></log4j:Message>",
            "</log4j:Event>",
        ];
        assert!(parser.feed(lines[0], false).is_none());
        assert!(parser.feed(lines[1], false).is_none());
        assert!(parser.feed(lines[2], false).is_none());
        let (level, line) = parser.feed(lines[3], false).unwrap();
        assert_eq!(level, "WARN");
        assert_eq!(line, "[Render thread/WARN] [Minecraft]: Hello\nworld");

        let (level, line) = parser.feed("plain stdout line", false).unwrap();
        assert_eq!((level.as_str(), line.as_str()), ("INFO", "plain stdout line"));
    }
}

fn parse_event(xml: &str) -> (String, String) {
    let head = &xml[..xml.find('>').unwrap_or(xml.len())];
    let level = attribute(head, "level").unwrap_or("INFO").to_string();
    let thread = attribute(head, "thread").unwrap_or("main");
    let logger = attribute(head, "logger").unwrap_or("");
    let logger = logger.rsplit('.').next().unwrap_or(logger);
    let message = between(xml, "<log4j:Message><![CDATA[", "]]></log4j:Message>").unwrap_or("");

    let mut line = if logger.is_empty() {
        format!("[{thread}/{level}]: {message}")
    } else {
        format!("[{thread}/{level}] [{logger}]: {message}")
    };
    if let Some(throwable) = between(xml, "<log4j:Throwable><![CDATA[", "]]></log4j:Throwable>") {
        line.push('\n');
        line.push_str(throwable.trim_end());
    }
    (level, line)
}
