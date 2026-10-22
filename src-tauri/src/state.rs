use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::oneshot;

use crate::paths::Paths;
use crate::versions::VersionManifest;

pub struct AppState {
    pub http: reqwest::Client,
    pub paths: Paths,
    /// Running games: instance id -> channel that kills the process.
    pub running: Mutex<HashMap<String, oneshot::Sender<()>>>,
    /// Instances that are currently being prepared (downloads, auth, ...).
    pub launching: Mutex<HashSet<String>>,
    /// Device code of the sign-in in progress; cleared to cancel it.
    pub login_device_code: Mutex<Option<String>>,
    pub manifest: tokio::sync::Mutex<Option<VersionManifest>>,
    /// Serialises reads and writes of accounts.json across token refreshes.
    pub accounts_lock: tokio::sync::Mutex<()>,
}

impl AppState {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("MineArcade-Launcher/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(300))
            .build()?;

        Ok(Self {
            http,
            paths: Paths::new()?,
            running: Mutex::new(HashMap::new()),
            launching: Mutex::new(HashSet::new()),
            login_device_code: Mutex::new(None),
            manifest: tokio::sync::Mutex::new(None),
            accounts_lock: tokio::sync::Mutex::new(()),
        })
    }
}
