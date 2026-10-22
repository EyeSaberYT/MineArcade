//! Global (non per-instance) launcher settings.

use serde::{Deserialize, Serialize};

use super::paths::{read_json, write_json, AppPaths};
use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Explicit Java executable for all instances (overrides auto-detection).
    pub java_path: Option<String>,
    /// Default memory allocation used when creating a new instance (MiB).
    pub default_memory_min: u32,
    pub default_memory_max: u32,
    /// Default JVM arguments prepended to every launch.
    #[serde(default)]
    pub default_jvm_args: Vec<String>,
    /// How many files to download in parallel.
    pub concurrent_downloads: usize,
    /// Enable Discord-style background update checks on startup.
    pub check_updates_on_start: bool,
    /// Show snapshots / betas in the version picker.
    pub show_snapshots: bool,
    /// Close the launcher window when the game starts.
    pub close_on_launch: bool,
    /// Keep the launcher running and show a "running" state instead.
    pub keep_launcher_open: bool,
    /// UI accent theme.
    pub theme: Theme,
    /// Modrinth API base URL (overridable for self-hosted Labrinth).
    pub modrinth_api_url: String,
    /// Server behind the Play page's "Join" button, and the entry every new
    /// instance gets in its in-game server list.
    #[serde(default = "default_featured_server")]
    pub featured_server: String,
}

/// The MineArcade network.
pub const DEFAULT_SERVER_ADDRESS: &str = "play.minearcade.org";
pub const DEFAULT_SERVER_NAME: &str = "MineArcade";

fn default_featured_server() -> String {
    DEFAULT_SERVER_ADDRESS.into()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            java_path: None,
            default_memory_min: 1024,
            default_memory_max: 4096,
            // Deliberately conservative, because these are passed to whatever
            // Java the game needs — currently Java 25 for Minecraft 26.x. Tuning
            // flags like -XX:G1NewSizePercent are *experimental* there and abort
            // the JVM unless -XX:+UnlockExperimentalVMOptions precedes them,
            // which is a poor trade for a few percent of GC tuning. G1 is the
            // default collector anyway, so only the encoding is worth setting.
            default_jvm_args: vec!["-Dfile.encoding=UTF-8".into()],
            concurrent_downloads: 8,
            check_updates_on_start: true,
            show_snapshots: false,
            close_on_launch: false,
            keep_launcher_open: true,
            theme: Theme::Dark,
            modrinth_api_url: "https://api.modrinth.com/v2".into(),
            featured_server: default_featured_server(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Dark,
    Light,
    System,
}

/// The Microsoft OAuth client id the launcher signs in with.
///
/// This is deliberately a constant rather than a setting. It is *not* a secret:
/// it is a public-client id, it ships inside the binary, and it is visible in
/// every OAuth URL the app requests. What matters is that it is not
/// user-editable, because pointing sign-in at an unregistered app only ever
/// produces confusing failures.
///
/// It can only be swapped at build time (`MINEARCADE_MS_CLIENT_ID=<id> cargo build`):
/// it is read with `option_env!`, so nothing at runtime — no setting, no
/// command, no config file — can change it.
pub fn microsoft_client_id() -> &'static str {
    // MineArcade's Azure app registration (public client, personal accounts only),
    // allow-listed by Mojang for Minecraft sign-in.
    option_env!("MINEARCADE_MS_CLIENT_ID").unwrap_or("25257470-05eb-4959-bc32-6b6bdb883bac")
}

impl Settings {
    pub fn load(paths: &AppPaths) -> Result<Self> {
        Ok(read_json(&paths.config_file())?.unwrap_or_default())
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        write_json(&paths.config_file(), self)
    }
}
