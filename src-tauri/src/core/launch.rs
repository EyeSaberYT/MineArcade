//! End-to-end launch pipeline: resolve → install → spawn → watch.
//!
//! This module is deliberately framework-agnostic; the Tauri command layer just
//! wires it up and forwards progress events to the UI.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

use super::auth::Account;
use super::config::Settings;
use super::download::{Progress, ProgressSink};
use super::instance::{self, Instance};
use super::java;
use super::minecraft::{self, LaunchContext};
use super::paths::{read_json, write_json, AppPaths};
use crate::error::{LauncherError, Result};

/// Everything needed to start a game process.
pub struct LaunchRequest<'a> {
    pub http: &'a reqwest::Client,
    pub paths: &'a AppPaths,
    pub instance: &'a Instance,
    pub account: &'a Account,
    pub settings: &'a Settings,
    pub sink: ProgressSink,
    /// Receives the game's console output and its exit notification.
    pub events: GameEventSink,
    /// Skip launching and just return the command (used by "preview command").
    pub dry_run: bool,
    /// Firing this (or dropping its sender) is ignored; sending `()` kills the game.
    pub stop: Option<tokio::sync::oneshot::Receiver<()>>,
    /// Which game this is, echoed back in its [`GameExit`] (an instance can run several times).
    pub session: u64,
}

/// A successfully started game process.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchedProcess {
    pub pid: u32,
    pub log_file: String,
    pub command: String,
    pub arguments: Vec<String>,
}

/// Which of the game's two output streams a line came from.
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// One line of game output, forwarded to the UI the moment it appears.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameOutput {
    pub instance_id: String,
    pub stream: OutputStream,
    pub line: String,
}

/// Sent once the game process has exited.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameExit {
    pub instance_id: String,
    /// The [`LaunchRequest::session`] of the game that exited.
    pub session: u64,
    /// Exit status, or `None` when the process was killed by a signal.
    pub code: Option<i32>,
    /// How long the game ran, in whole seconds: added to the instance's play time.
    pub play_secs: u64,
}

/// Something a running game told us: a line of output, or that it stopped.
pub enum GameEvent {
    Output(GameOutput),
    Exit(GameExit),
}

/// Receives [`GameEvent`]s for the lifetime of a game process.
pub type GameEventSink = Arc<dyn Fn(GameEvent) + Send + Sync>;

/// A sink that discards every event — used for dry runs and previews.
pub fn silent_events() -> GameEventSink {
    Arc::new(|_| {})
}

/// Run the full pipeline and start the game.
pub async fn launch(request: LaunchRequest<'_>) -> Result<LaunchedProcess> {
    let LaunchRequest {
        http,
        paths,
        instance,
        account,
        settings,
        sink,
        events,
        dry_run,
        stop,
        session,
    } = request;

    if account.is_expired() {
        return Err(LauncherError::Auth(
            "Your session expired. Please sign in again.".into(),
        ));
    }

    let game_dir = instance::game_dir(paths, instance);
    std::fs::create_dir_all(&game_dir)?;

    // ---- 1. resolve the version ------------------------------------------
    sink(Progress::new("resolve", "Resolving game version…"));
    let version = minecraft::resolve(
        http,
        paths,
        &instance.game_version,
        instance.loader,
        instance.loader_version.as_deref(),
    )
    .await?;

    // ---- 2. make sure Java is available ----------------------------------
    let required = version
        .java_version
        .as_ref()
        .map(|j| java::required_major(&j.component, j.major_version))
        .unwrap_or(8);

    sink(Progress::new("java", format!("Looking for Java {required}…")));
    let explicit = instance
        .settings
        .java_path
        .clone()
        .or_else(|| settings.java_path.clone());

    let mut runtime = java::detect(paths, explicit.as_deref(), required).await;
    if runtime.as_ref().map(|r| r.major < required).unwrap_or(true) {
        sink(Progress::new(
            "java",
            format!("Java {required} is missing — downloading it for you…"),
        ));
        runtime = Some(
            java::install(http, paths, required, sink.clone())
                .await
                .map_err(|e| LauncherError::msg(format!("While setting up Java: {e}")))?,
        );
    }
    let runtime = runtime.ok_or_else(|| {
        LauncherError::msg(format!(
            "Java {required} is required but couldn't be found or installed."
        ))
    })?;

    // ---- 3. install files ------------------------------------------------
    sink(Progress::new("install", "Checking game files…"));
    let installed = minecraft::ensure_files(
        http,
        paths,
        &version,
        &instance.game_version,
        settings.concurrent_downloads.max(1),
        sink.clone(),
    )
    .await
    .map_err(|e| LauncherError::msg(format!("While downloading the game files: {e}")))?;

    // ---- 4. build the command -------------------------------------------
    let mut jvm_args = settings.default_jvm_args.clone();
    jvm_args.extend(instance.settings.jvm_args.clone());

    let ctx = LaunchContext {
        version: installed.version.clone(),
        base_version: instance.game_version.clone(),
        game_dir: game_dir.clone(),
        assets_dir: paths.assets_dir(),
        asset_index: installed.asset_index.clone(),
        natives_dir: installed.natives_dir.clone(),
        classpath: installed.classpath.clone(),
        username: account.username.clone(),
        uuid: account.uuid.replace('-', ""),
        access_token: account.access_token.clone(),
        user_type: account.user_type().to_string(),
        width: instance.settings.width,
        height: instance.settings.height,
        fullscreen: instance.settings.fullscreen,
        memory_min: instance.settings.memory_min,
        memory_max: instance.settings.memory_max,
        extra_jvm_args: jvm_args,
        extra_game_args: instance.settings.game_args.clone(),
        quick_play: instance.settings.quick_play.clone(),
        logging_arg: installed.logging_arg.clone(),
    };

    let (_program, args) = minecraft::build_command(&ctx)?;
    let java_path = runtime.as_path();

    if dry_run {
        return Ok(LaunchedProcess {
            pid: 0,
            log_file: String::new(),
            command: java_path.to_string_lossy().to_string(),
            arguments: args,
        });
    }

    // ---- 5. spawn --------------------------------------------------------
    sink(Progress::new("launch", "Starting Minecraft…"));

    let logs_dir = paths.instance_dir(&instance.id).join("logs");
    std::fs::create_dir_all(&logs_dir)?;
    let log_file = logs_dir.join(format!(
        "launch-{}.log",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    ));

    let mut command = tokio::process::Command::new(&java_path);
    command
        .args(&args)
        .current_dir(&game_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    for (key, value) in &instance.settings.env {
        command.env(key, value);
    }

    super::no_console(&mut command);
    own_process_group(&mut command);

    // Keep the whole transcript on disk for the crash viewer, while relaying
    // every line to the UI as it arrives. Both streams share one handle so
    // stdout and stderr stay interleaved in the order they happened. Opened
    // before the spawn: failing afterwards would leave a game nobody watches.
    let log_handle = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file)
        .await
        .map_err(|e| LauncherError::msg(format!("Couldn't open {}: {e}", log_file.display())))?;
    let log_handle = Arc::new(tokio::sync::Mutex::new(log_handle));

    // Stop pressed while the game was still being set up: don't start it at all.
    let mut stop = stop;
    if let Some(receiver) = stop.as_mut() {
        if receiver.try_recv().is_ok() {
            return Err(LauncherError::Launch("The launch was cancelled.".into()));
        }
    }

    let mut child = command
        .spawn()
        .map_err(|e| LauncherError::Launch(format!("Couldn't start Java: {e}")))?;

    let pid = child.id().unwrap_or(0);
    let started = std::time::Instant::now();
    let running = RunningGame {
        instance_id: instance.id.clone(),
        pid,
        started: chrono::Utc::now().timestamp(),
    };
    if pid != 0 {
        record_running(paths, &running);
    }

    if let Some(stdout) = child.stdout.take() {
        pump_output(
            stdout,
            OutputStream::Stdout,
            instance.id.clone(),
            events.clone(),
            log_handle.clone(),
        );
    }
    if let Some(stderr) = child.stderr.take() {
        pump_output(
            stderr,
            OutputStream::Stderr,
            instance.id.clone(),
            events.clone(),
            log_handle.clone(),
        );
    }

    // Reap the child in the background so we don't leave a zombie.
    let watched = instance.id.clone();
    let paths = paths.clone();
    tokio::spawn(async move {
        let status = match stop {
            Some(mut stop) => tokio::select! {
                status = child.wait() => status,
                Ok(()) = &mut stop => {
                    log::info!("stopping instance {watched} (pid {pid}) on request");
                    kill_tree(pid).await;
                    let _ = child.kill().await;
                    child.wait().await
                }
            },
            None => child.wait().await,
        };
        match &status {
            Ok(status) => log::info!("instance {watched} exited with {status}"),
            Err(error) => log::warn!("failed to wait for instance {watched}: {error}"),
        }
        forget_running(&paths, &running);
        // The play time goes to the launcher's in-memory instance store (see AppState::game_sink),
        // which is what the UI reads and what gets saved; editing a fresh copy from disk here
        // would be invisible to it and overwritten by its next save.
        events(GameEvent::Exit(GameExit {
            instance_id: watched.clone(),
            session,
            code: status.ok().and_then(|status| status.code()),
            play_secs: started.elapsed().as_secs(),
        }));
    });

    sink(Progress {
        stage: "launch".into(),
        label: "Minecraft is running".into(),
        current: 1,
        total: 1,
        files_done: 1,
        files_total: 1,
        finished: true,
    });

    // Strip tokens from the returned argument list before passing to the
    // frontend (they aren't needed for display and shouldn't be exposed).
    let mut safe_args = args.clone();
    for a in &mut safe_args {
        if !account.access_token.is_empty() && a.contains(&account.access_token) {
            *a = a.replace(&account.access_token, "****");
        }
        if let Some(rt) = &account.refresh_token {
            if !rt.is_empty() && a.contains(rt) {
                *a = a.replace(rt, "****");
            }
        }
        if let Some(idx) = a.find("token:") {
            let rest = &a[idx + 6..];
            if !rest.is_empty() && rest.len() > 4 {
                a.replace_range(idx + 6.., "****");
            } else {
                *a = a.replace(&a[idx..], "token:****");
            }
        }
    }

    Ok(LaunchedProcess {
        pid,
        log_file: log_file.to_string_lossy().to_string(),
        command: java_path.to_string_lossy().to_string(),
        arguments: safe_args,
    })
}

/// On unix, start the game as the leader of its own process group, so [`kill_tree`]
/// can close it together with whatever it started. Windows uses `taskkill /T` instead.
pub(crate) fn own_process_group(command: &mut tokio::process::Command) {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(not(unix))]
    let _ = command;
}

/// Force-close a game and everything it started (on Windows, `taskkill /T /F`; on unix,
/// SIGKILL to the process group [`own_process_group`] gave it). Killing only the Java
/// process could leave processes it spawned behind.
pub async fn kill_tree(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(windows)]
    let mut command = {
        let mut command = tokio::process::Command::new("taskkill");
        command.args(["/PID", &pid.to_string(), "/T", "/F"]);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        // A negative pid names the process group (whose id is the leader's pid).
        let mut command = tokio::process::Command::new("kill");
        command.args(["-9", "--", &format!("-{pid}")]);
        command
    };
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    super::no_console(&mut command);
    match command.status().await {
        Ok(status) if status.success() => {}
        Ok(status) => log::warn!("killing game process {pid} returned {status}"),
        Err(error) => log::warn!("couldn't kill game process {pid}: {error}"),
    }
}

// ---------------------------------------------------------------------------
// Games that outlive the launcher
// ---------------------------------------------------------------------------

/// A started game, kept in `<root>/running.json` until the launcher sees it exit.
///
/// Play time is added when the watcher in [`launch`] sees the game exit, but the game
/// keeps running when the launcher quits first (`closeOnLaunch` quits right after a
/// launch). The next launcher start finds these leftovers and credits them with
/// [`orphan_play_secs`] once [`java_is_running`] says they're gone.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningGame {
    pub instance_id: String,
    pub pid: u32,
    /// Unix seconds.
    pub started: i64,
}

/// Serialises the read-modify-writes of `running.json`.
static RUNNING_FILE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn read_running(paths: &AppPaths) -> Vec<RunningGame> {
    read_json(&paths.running_file()).ok().flatten().unwrap_or_default()
}

fn update_running(paths: &AppPaths, change: impl FnOnce(&mut Vec<RunningGame>)) {
    let _guard = RUNNING_FILE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut games = read_running(paths);
    change(&mut games);
    if let Err(error) = write_json(&paths.running_file(), &games) {
        log::warn!("couldn't update the list of running games: {error}");
    }
}

pub fn record_running(paths: &AppPaths, game: &RunningGame) {
    update_running(paths, |games| games.push(game.clone()));
}

pub fn forget_running(paths: &AppPaths, game: &RunningGame) {
    update_running(paths, |games| games.retain(|g| g != game));
}

/// Every recorded game; read at startup, before this launcher has started any.
pub fn load_running(paths: &AppPaths) -> Vec<RunningGame> {
    let _guard = RUNNING_FILE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    read_running(paths)
}

/// Whether `pid` is still a running Java process. The name check keeps a pid the OS
/// gave to some other program from passing for the game; a later Java reusing it only
/// delays the credit, since [`orphan_play_secs`] goes by the game's own log.
pub async fn java_is_running(pid: u32) -> bool {
    #[cfg(windows)]
    let mut command = {
        let mut command = tokio::process::Command::new("tasklist");
        command.args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"]);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = tokio::process::Command::new("ps");
        command.args(["-p", &pid.to_string(), "-o", "comm="]);
        command
    };
    command.stdin(Stdio::null()).stderr(Stdio::null());
    super::no_console(&mut command);
    let Ok(output) = command.output().await else {
        return false;
    };
    // tasklist: `"java.exe","1234",…`; ps: the command name, empty once it's gone.
    let quoted_pid = format!("\"{pid}\"");
    String::from_utf8_lossy(&output.stdout)
        .to_lowercase()
        .lines()
        .any(|line| line.contains("java") && (cfg!(not(windows)) || line.contains(&quoted_pid)))
}

/// Seconds a game the launcher never saw exit was played: from its start until Minecraft
/// last wrote `logs/latest.log` in its game folder, which it does until it closes. The
/// real exit time is unknown, so this is the closest stand-in; without a log written
/// since the start it's 0. Copies of one instance share that log, so overlapping copies
/// can be over-credited.
pub fn orphan_play_secs(game: &RunningGame, game_dir: &Path) -> u64 {
    let last_write = std::fs::metadata(game_dir.join("logs").join("latest.log"))
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_secs() as i64);
    played_between(game.started, last_write, chrono::Utc::now().timestamp())
}

fn played_between(started: i64, last_write: Option<i64>, now: i64) -> u64 {
    match last_write {
        Some(end) if end > started => (end.min(now) - started) as u64,
        _ => 0,
    }
}

/// Relay one of the game's output streams to the log file and to the UI.
///
/// Reading line by line instead of copying raw bytes is what makes the console
/// live: each complete line is pushed to the frontend the moment it arrives,
/// while the same text is appended to the log for the crash viewer.
fn pump_output<R>(
    reader: R,
    stream: OutputStream,
    instance_id: String,
    events: GameEventSink,
    log_handle: Arc<tokio::sync::Mutex<tokio::fs::File>>,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        // Read raw bytes rather than `lines()`: Java can print text that isn't
        // UTF-8, and `lines()` gives up on the first such line — after which
        // nobody drains the pipe and the game freezes once its buffer fills.
        let mut reader = tokio::io::BufReader::new(reader);
        let mut buffer = Vec::new();
        let mut parser = LogParser::default();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let text = String::from_utf8_lossy(&buffer);
            let raw = text.trim_end_matches(|c: char| c == '\r' || c == '\n');
            // Mojang's logging config prints XML events; turn them back into lines.
            let Some(line) = parser.feed(raw) else { continue };
            let mut redacted = line.clone();
            {
                let mut file = log_handle.lock().await;
                if redacted.contains("access_token") || redacted.contains("auth_access_token") {
                    // best-effort mask common token patterns
                    if let Some(pos) = redacted.find("token:") {
                        let end = redacted[pos + 6..].find(|c: char| c.is_whitespace() || c == '"' || c == '\'').map(|p| pos + 6 + p).unwrap_or(redacted.len());
                        redacted.replace_range(pos + 6..end, "****");
                    }
                    if let Some(pos) = redacted.find("access_token") {
                        let mut start = pos + "access_token".len();
                        if redacted[start..].starts_with('=') { start += 1; }
                        let end = redacted[start..].find(|c: char| c.is_whitespace() || c == '"' || c == '\'').map(|p| start + p).unwrap_or(redacted.len());
                        if end > start { redacted.replace_range(start..end, "****"); }
                    }
                }
                let _ = file.write_all(redacted.as_bytes()).await;
                let _ = file.write_all(b"\n").await;
                let _ = file.flush().await;
            }
            let emitted = redacted.clone();
            events(GameEvent::Output(GameOutput {
                instance_id: instance_id.clone(),
                stream,
                line: emitted,
            }));
        }
    });
}

/// Turns the XML events printed under Mojang's log4j config back into the
/// `[thread/LEVEL] [logger]: message` lines players expect. Anything that isn't
/// an event (mod and JVM output) passes through untouched.
#[derive(Default)]
pub(crate) struct LogParser {
    pending: Option<String>,
}

impl LogParser {
    pub(crate) fn feed(&mut self, line: &str) -> Option<String> {
        if let Some(pending) = self.pending.as_mut() {
            pending.push('\n');
            pending.push_str(line);
            if line.contains("</log4j:Event>") {
                let event = self.pending.take().unwrap_or_default();
                return Some(format_event(&event));
            }
            return None;
        }
        if line.trim_start().starts_with("<log4j:Event") {
            if line.contains("</log4j:Event>") {
                return Some(format_event(line));
            }
            self.pending = Some(line.to_string());
            return None;
        }
        Some(line.to_string())
    }
}

fn xml_attribute<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    let start = head.find(&key)? + key.len();
    let end = head[start..].find('"')? + start;
    Some(&head[start..end])
}

fn xml_between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = text.find(start)? + start.len();
    let to = text[from..].find(end)? + from;
    Some(&text[from..to])
}

fn format_event(xml: &str) -> String {
    let head = &xml[..xml.find('>').unwrap_or(xml.len())];
    let level = xml_attribute(head, "level").unwrap_or("INFO");
    let thread = xml_attribute(head, "thread").unwrap_or("main");
    let logger = xml_attribute(head, "logger").unwrap_or("");
    let logger = logger.rsplit('.').next().unwrap_or(logger);
    let message = xml_between(xml, "<log4j:Message><![CDATA[", "]]></log4j:Message>").unwrap_or("");

    let mut line = if logger.is_empty() {
        format!("[{thread}/{level}]: {message}")
    } else {
        format!("[{thread}/{level}] [{logger}]: {message}")
    };
    if let Some(throwable) = xml_between(xml, "<log4j:Throwable><![CDATA[", "]]></log4j:Throwable>") {
        line.push('\n');
        line.push_str(throwable.trim_end());
    }
    line
}

#[cfg(test)]
mod log_parser_tests {
    use super::LogParser;

    #[test]
    fn turns_xml_events_back_into_lines() {
        let mut parser = LogParser::default();
        assert!(parser
            .feed(r#"<log4j:Event logger="net.minecraft.client.Minecraft" timestamp="1" level="WARN" thread="Render thread">"#)
            .is_none());
        assert!(parser.feed("  <log4j:Message><![CDATA[Hello").is_none());
        assert!(parser.feed("world]]></log4j:Message>").is_none());
        assert_eq!(
            parser.feed("</log4j:Event>").as_deref(),
            Some("[Render thread/WARN] [Minecraft]: Hello\nworld")
        );
        assert_eq!(parser.feed("plain output").as_deref(), Some("plain output"));
    }
}

/// Read the tail of the most recent log for an instance.
pub fn read_latest_log(paths: &AppPaths, instance_id: &str, lines: usize) -> Result<Option<String>> {
    let logs_dir: PathBuf = paths.instance_dir(instance_id).join("logs");
    let mut entries: Vec<_> = std::fs::read_dir(&logs_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "log")
                .unwrap_or(false)
        })
        .collect();

    entries.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
    let Some(latest) = entries.last() else {
        return Ok(None);
    };

    let content = std::fs::read_to_string(latest.path())?;
    let tail: Vec<&str> = content.lines().rev().take(lines).collect();
    Ok(Some(tail.into_iter().rev().collect::<Vec<_>>().join("\n")))
}

/// Loader + version summary for the instance card.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceSummary {
    pub id: String,
    pub name: String,
    pub game_version: String,
    pub loader: String,
    pub loader_version: Option<String>,
    pub icon: Option<String>,
    pub last_played: Option<String>,
    pub play_time_secs: u64,
    pub mod_count: usize,
    pub installed: bool,
    /// Free-form tags; orbit-client marks the managed Orbit Client instance.
    pub tags: Vec<String>,
}

/// Build a UI-friendly summary for an instance.
pub fn summarize(paths: &AppPaths, instance: &Instance) -> InstanceSummary {
    let game_dir = instance::game_dir(paths, instance);
    let mods = game_dir.join("mods");
    let mod_count = std::fs::read_dir(&mods)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "jar")
                .unwrap_or(false)
        })
        .count();

    InstanceSummary {
        id: instance.id.clone(),
        name: instance.name.clone(),
        game_version: instance.game_version.clone(),
        loader: instance.loader.as_str().to_string(),
        loader_version: instance.loader_version.clone(),
        icon: instance.icon.clone(),
        last_played: instance.last_played.clone(),
        play_time_secs: instance.play_time_secs,
        mod_count,
        installed: minecraft::resolve_cached(paths, instance).is_some(),
        tags: instance.tags.clone(),
    }
}

#[cfg(test)]
mod kill_tests {
    use super::kill_tree;

    /// Stop must close a running process (and what it started), not just ask it to.
    #[tokio::test]
    async fn kill_tree_closes_a_running_process() {
        #[cfg(windows)]
        let mut command = {
            // cmd starts ping as its own child: the tree a game with helper processes has.
            let mut command = tokio::process::Command::new("cmd");
            command.args(["/c", "ping -n 60 127.0.0.1 > NUL"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = tokio::process::Command::new("sh");
            command.args(["-c", "sleep 60"]);
            command
        };
        // Started the way games are, so unix kills it through its process group.
        super::own_process_group(&mut command);
        let mut child = command.spawn().expect("start a long-running process");
        kill_tree(child.id().expect("pid")).await;
        let status = tokio::time::timeout(std::time::Duration::from_secs(10), child.wait())
            .await
            .expect("the process is still running after kill_tree")
            .expect("wait for the process");
        assert!(!status.success());
    }
}

#[cfg(test)]
mod running_tests {
    use super::*;

    #[test]
    fn running_games_are_recorded_until_forgotten() {
        let root = std::env::temp_dir().join(format!("orbit-launcher-running-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = AppPaths::new(&root);
        let game = |pid| RunningGame { instance_id: "abc".into(), pid, started: 100 };

        record_running(&paths, &game(1));
        record_running(&paths, &game(2));
        forget_running(&paths, &game(1));
        assert_eq!(load_running(&paths), vec![game(2)]);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn orphaned_play_time_runs_until_the_last_log_write() {
        assert_eq!(played_between(100, Some(160), 1000), 60);
        // A log from before the start belongs to an earlier game.
        assert_eq!(played_between(100, Some(90), 1000), 0);
        assert_eq!(played_between(100, None, 1000), 0);
        // A clock jump never credits time that hasn't happened yet.
        assert_eq!(played_between(100, Some(5000), 1000), 900);
    }

    #[tokio::test]
    async fn only_a_live_java_process_counts_as_the_game() {
        // This test binary is alive but isn't Java: a reused pid must not pass for the game.
        assert!(!java_is_running(std::process::id()).await);
        let mut child = if cfg!(windows) {
            tokio::process::Command::new("cmd").args(["/c", "exit 0"]).spawn()
        } else {
            tokio::process::Command::new("true").spawn()
        }
        .expect("start a short process");
        let pid = child.id().expect("pid");
        child.wait().await.expect("wait");
        assert!(!java_is_running(pid).await);
    }
}

/// Starts a real modded game in a scratch folder and watches it get to the title screen.
/// Downloads the game (a few minutes). Run with ORBIT_LIVE_JAVA (Java 17+ for the installer),
/// ORBIT_LIVE_LOADER (forge | neoforge), ORBIT_LIVE_GAME and ORBIT_LIVE_LOADER_VERSION:
/// `cargo test --lib live_loader_launch -- --ignored --nocapture`
#[cfg(test)]
mod live_launch_tests {
    use super::*;
    use crate::core::auth::AccountKind;
    use crate::core::instance::LoaderKind;

    #[tokio::test]
    #[ignore]
    async fn live_loader_launch() {
        let env = |key: &str| std::env::var(key).unwrap_or_else(|_| panic!("set {key}"));
        let (java, game, loader_version) = (env("ORBIT_LIVE_JAVA"), env("ORBIT_LIVE_GAME"), env("ORBIT_LIVE_LOADER_VERSION"));
        let loader = match env("ORBIT_LIVE_LOADER").as_str() {
            "neoforge" => LoaderKind::NeoForge,
            "fabric" => LoaderKind::Fabric,
            _ => LoaderKind::Forge,
        };
        let root = std::env::temp_dir().join(format!("orbit-live-launch-{}-{game}", loader.as_str()));
        let paths = AppPaths::new(&root);
        paths.ensure().unwrap();
        let http = reqwest::Client::builder().user_agent("orbit-launcher-test").build().unwrap();
        crate::core::minecraft::loader::install_loader(&http, &paths, &root, loader, &game, &loader_version, std::path::Path::new(&java), crate::core::download::silent())
            .await
            .expect("install the loader");

        let mut instance = Instance::new("live", game.clone(), loader);
        instance.loader_version = Some(loader_version.clone());
        // The game's Java: as the launcher picks it (downloading it if needed), unless forced.
        instance.settings.java_path = std::env::var("ORBIT_LIVE_GAME_JAVA").ok();
        let account = Account {
            kind: AccountKind::Microsoft,
            username: "OrbitTest".into(),
            uuid: "00000000000000000000000000000001".into(),
            access_token: "0".into(),
            expires_at: chrono::Utc::now().timestamp() + 3600,
            refresh_token: None,
            skin_url: None,
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let events: GameEventSink = Arc::new(move |event| {
            let _ = tx.send(event);
        });
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let settings = Settings::default();
        let launched = launch(LaunchRequest {
            http: &http,
            paths: &paths,
            instance: &instance,
            account: &account,
            settings: &settings,
            sink: crate::core::download::silent(),
            events,
            dry_run: false,
            stop: Some(stop_rx),
            session: 1,
        })
        .await;
        let launched = launched.expect("start the game");
        println!("started pid {} (log {})", launched.pid, launched.log_file);

        // The title screen's resources are loading once the sound engine starts.
        let mut reached = false;
        let mut last = Vec::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
        let mut stop_tx = Some(stop_tx);
        loop {
            let event = match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(event)) => event,
                _ => break,
            };
            match event {
                GameEvent::Output(output) => {
                    let line = output.line;
                    if line.contains("Exception") || line.contains("Error") || line.contains("FATAL") || line.contains("Sound engine started") {
                        println!("| {}", line.chars().take(300).collect::<String>());
                    }
                    last.push(line.clone());
                    if last.len() > 40 {
                        last.remove(0);
                    }
                    if line.contains("Sound engine started") && !reached {
                        reached = true;
                        if let Some(stop) = stop_tx.take() {
                            let _ = stop.send(());
                        }
                    }
                }
                GameEvent::Exit(exit) => {
                    println!("exited with {:?} after {}s", exit.code, exit.play_secs);
                    break;
                }
            }
        }
        if !reached {
            if let Some(stop) = stop_tx.take() {
                let _ = stop.send(());
            }
            println!("--- last lines:");
            for line in &last {
                println!("| {}", line.chars().take(400).collect::<String>());
            }
        }
        assert!(reached, "the game didn't reach the title screen");
    }
}
