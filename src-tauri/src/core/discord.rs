//! Discord Rich Presence: shows "Playing MineArcade" with the instance being
//! played on the player's Discord profile.
//!
//! The IPC client is blocking, so it lives on its own thread and is driven by
//! messages. Discord may not be running (or may be restarted), so the thread
//! reconnects quietly every [`RETRY_EVERY`] and never reports errors to the UI.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

use discord_rich_presence::activity::{Activity, Assets, Button, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};

use super::instance::{Instance, LoaderKind, QuickPlay};

/// How often a missing Discord connection is retried.
const RETRY_EVERY: Duration = Duration::from_secs(15);
/// Asset key of the logo uploaded under Rich Presence → Art Assets in the
/// Discord application.
const LARGE_IMAGE: &str = "minearcade-discord-logo";
/// The first of the two buttons Discord allows; always shown.
const DISCORD_INVITE: (&str, &str) = ("Join Discord", "https://discord.gg/5D7DrptJ75");
/// Discord rejects button labels longer than this.
const MAX_LABEL_CHARS: usize = 32;
const MAX_URL_CHARS: usize = 512;

/// The second, user-configurable button (Settings → General).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomButton {
    pub label: String,
    pub url: String,
}

impl CustomButton {
    /// A button Discord will accept, or `None` (left out) when the label is
    /// blank or the link isn't an http(s) URL.
    pub fn new(label: &str, url: &str) -> Option<Self> {
        let label: String = label.trim().chars().take(MAX_LABEL_CHARS).collect();
        let url = url.trim();
        let web = url.starts_with("https://") || url.starts_with("http://");
        (!label.is_empty() && web && url.len() <= MAX_URL_CHARS).then(|| Self { label, url: url.to_string() })
    }
}

/// The buttons under the presence: Join Discord, then the custom one if set.
fn buttons(custom: Option<&CustomButton>) -> Vec<(String, String)> {
    let mut list = vec![(DISCORD_INVITE.0.to_string(), DISCORD_INVITE.1.to_string())];
    if let Some(button) = custom {
        list.push((button.label.clone(), button.url.clone()));
    }
    list
}

/// The "MineArcade" application on the Discord Developer Portal (IDs are
/// public). Build with `MINEARCADE_DISCORD_APP_ID=<id>` to swap it; an empty
/// value turns Rich Presence off entirely.
pub fn app_id() -> &'static str {
    option_env!("MINEARCADE_DISCORD_APP_ID").unwrap_or("1566135647021043893")
}

/// What one running game shows.
#[derive(Debug, Clone)]
pub struct Game {
    pub instance_id: String,
    pub name: String,
    pub game_version: String,
    pub loader: LoaderKind,
    /// Server joined through Quick Play, if any.
    pub server: Option<String>,
    /// Unix seconds, for Discord's "elapsed" timer.
    pub started: i64,
}

impl Game {
    pub fn from_instance(instance: &Instance) -> Self {
        let server = match &instance.settings.quick_play {
            QuickPlay::Multiplayer { address } if !address.trim().is_empty() => Some(address.trim().to_string()),
            _ => None,
        };
        Self {
            instance_id: instance.id.clone(),
            name: instance.name.clone(),
            game_version: instance.game_version.clone(),
            loader: instance.loader,
            server,
            started: chrono::Utc::now().timestamp(),
        }
    }
}

enum Message {
    Options { enabled: bool, custom: Option<CustomButton> },
    Started(Game),
    Stopped(String),
}

/// Cheap handle to the presence thread; every method is fire-and-forget.
#[derive(Clone)]
pub struct Presence {
    tx: Option<Sender<Message>>,
}

impl Presence {
    /// Start the presence thread (a no-op handle when no app ID is built in).
    pub fn start(enabled: bool, custom: Option<CustomButton>) -> Self {
        let id = app_id().trim();
        if id.is_empty() {
            log::info!("Discord Rich Presence is off: no application ID in this build");
            return Self { tx: None };
        }
        let (tx, rx) = mpsc::channel();
        let id = id.to_string();
        let spawned = std::thread::Builder::new()
            .name("discord-presence".into())
            .spawn(move || run(&id, enabled, custom, rx));
        match spawned {
            Ok(_) => Self { tx: Some(tx) },
            Err(error) => {
                log::warn!("couldn't start Discord Rich Presence: {error}");
                Self { tx: None }
            }
        }
    }

    /// Apply the Settings: on/off and the custom second button.
    pub fn set_options(&self, enabled: bool, custom: Option<CustomButton>) {
        self.send(Message::Options { enabled, custom });
    }

    pub fn game_started(&self, game: Game) {
        self.send(Message::Started(game));
    }

    pub fn game_stopped(&self, instance_id: &str) {
        self.send(Message::Stopped(instance_id.to_string()));
    }

    fn send(&self, message: Message) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(message);
        }
    }
}

fn loader_label(loader: LoaderKind) -> &'static str {
    match loader {
        LoaderKind::Vanilla => "Vanilla",
        LoaderKind::Fabric => "Fabric",
        LoaderKind::Quilt => "Quilt",
        LoaderKind::Forge => "Forge",
        LoaderKind::NeoForge => "NeoForge",
    }
}

/// The two lines shown under the app name: what is being played, then where.
fn lines(game: Option<&Game>) -> (String, String) {
    match game {
        Some(game) => (
            format!("Playing {}", game.name),
            match &game.server {
                Some(server) => format!("On {server}"),
                None => format!("Minecraft {} · {}", game.game_version, loader_label(game.loader)),
            },
        ),
        None => ("In the launcher".into(), "Picking what to play".into()),
    }
}

fn run(app_id: &str, mut enabled: bool, mut custom: Option<CustomButton>, rx: mpsc::Receiver<Message>) {
    let launcher_started = chrono::Utc::now().timestamp();
    // Most recently started last; the newest game is the one shown.
    let mut games: Vec<Game> = Vec::new();
    let mut client: Option<DiscordIpcClient> = None;

    loop {
        // Push the current state, connecting first when needed.
        if enabled {
            if client.is_none() {
                let mut fresh = DiscordIpcClient::new(app_id);
                match fresh.connect() {
                    Ok(()) => {
                        log::info!("connected to Discord for Rich Presence");
                        client = Some(fresh);
                    }
                    // Discord isn't running: try again later, silently.
                    Err(error) => log::debug!("Discord not reachable: {error}"),
                }
            }
            if let Some(connected) = client.as_mut() {
                let game = games.last();
                let (details, state) = lines(game);
                let links = buttons(custom.as_ref());
                let activity = Activity::new()
                    .details(details)
                    .state(state)
                    .timestamps(Timestamps::new().start(game.map_or(launcher_started, |g| g.started)))
                    .assets(Assets::new().large_image(LARGE_IMAGE).large_text("MineArcade"))
                    .buttons(links.iter().map(|(label, url)| Button::new(label.as_str(), url.as_str())).collect());
                if let Err(error) = connected.set_activity(activity) {
                    // Discord quit or restarted: drop the pipe and reconnect next round.
                    log::info!("lost the Discord connection: {error}");
                    let _ = connected.close();
                    client = None;
                }
            }
        } else if let Some(mut connected) = client.take() {
            let _ = connected.clear_activity();
            let _ = connected.close();
        }

        // Wait for a change. Meanwhile retry a missing connection, and re-send
        // a live one now and then so a restarted Discord is noticed.
        let wait = match (enabled, client.is_some()) {
            (true, false) => RETRY_EVERY,
            (true, true) => Duration::from_secs(60),
            (false, _) => Duration::from_secs(3600),
        };
        match rx.recv_timeout(wait) {
            Ok(Message::Options { enabled: on, custom: button }) => {
                enabled = on;
                custom = button;
            }
            Ok(Message::Started(game)) => {
                games.retain(|g| g.instance_id != game.instance_id);
                games.push(game);
            }
            Ok(Message::Stopped(id)) => games.retain(|g| g.instance_id != id),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    if let Some(mut connected) = client {
        let _ = connected.clear_activity();
        let _ = connected.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(server: Option<&str>) -> Game {
        Game {
            instance_id: "atm".into(),
            name: "All the Mods 10".into(),
            game_version: "1.21.1".into(),
            loader: LoaderKind::NeoForge,
            server: server.map(Into::into),
            started: 0,
        }
    }

    #[test]
    fn join_discord_comes_first_and_the_custom_button_is_optional() {
        assert_eq!(buttons(None), vec![("Join Discord".into(), "https://discord.gg/5D7DrptJ75".into())]);
        let custom = CustomButton::new("  Get MineArcade ", " https://example.com/dl ").unwrap();
        assert_eq!(buttons(Some(&custom))[1], ("Get MineArcade".into(), "https://example.com/dl".into()));
        // Blank labels and non-web links are dropped; long labels are cut to Discord's limit.
        assert_eq!(CustomButton::new("", "https://example.com"), None);
        assert_eq!(CustomButton::new("Site", "javascript:alert(1)"), None);
        assert_eq!(CustomButton::new(&"x".repeat(40), "https://example.com").unwrap().label.len(), 32);
    }

    #[test]
    fn presence_lines_describe_what_is_played() {
        assert_eq!(
            lines(Some(&game(None))),
            ("Playing All the Mods 10".into(), "Minecraft 1.21.1 · NeoForge".into())
        );
        assert_eq!(
            lines(Some(&game(Some("play.minearcade.org")))).1,
            "On play.minearcade.org"
        );
        assert_eq!(lines(None).0, "In the launcher");
    }
}
