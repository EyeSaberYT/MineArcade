use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::paths::Paths;

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Reads a JSON file, falling back to the default when it is missing. A corrupt file is
/// kept as `<name>.bak` so nothing is silently lost.
fn read_json<T: DeserializeOwned + Default>(path: &Path) -> T {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            let _ = std::fs::copy(path, path.with_extension("json.bak"));
            T::default()
        }),
        Err(_) => T::default(),
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)
        .with_context(|| format!("could not write {}", path.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("could not write {}", path.display()))?;
    Ok(())
}

// ---------- Settings ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Azure application (client) ID used for Microsoft sign-in.
    pub client_id: String,
    /// Server shown as a "Join" button on the Play page.
    pub featured_server: String,
    pub default_memory_mb: u32,
    pub hide_on_launch: bool,
    pub selected_instance: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            featured_server: String::new(),
            default_memory_mb: 4096,
            hide_on_launch: false,
            selected_instance: None,
        }
    }
}

impl Settings {
    /// The client ID from Settings, or the one baked in at build time via MINEARCADE_CLIENT_ID.
    pub fn effective_client_id(&self) -> String {
        let configured = self.client_id.trim();
        if !configured.is_empty() {
            return configured.to_string();
        }
        option_env!("MINEARCADE_CLIENT_ID").unwrap_or_default().trim().to_string()
    }
}

pub fn load_settings(paths: &Paths) -> Settings {
    read_json(&paths.settings_file())
}

pub fn save_settings(paths: &Paths, settings: &Settings) -> Result<()> {
    write_json(&paths.settings_file(), settings)
}

// ---------- Instances ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Loader {
    Vanilla,
    Fabric,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Instance {
    pub id: String,
    pub name: String,
    pub mc_version: String,
    pub loader: Loader,
    #[serde(default)]
    pub loader_version: Option<String>,
    pub memory_mb: u32,
    #[serde(default)]
    pub jvm_args: String,
    #[serde(default)]
    pub java_path: Option<String>,
    /// Server to join automatically when this instance starts.
    #[serde(default)]
    pub auto_join: Option<String>,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub last_played: Option<u64>,
}

pub fn load_instances(paths: &Paths) -> Vec<Instance> {
    read_json(&paths.instances_file())
}

pub fn save_instances(paths: &Paths, instances: &[Instance]) -> Result<()> {
    write_json(&paths.instances_file(), &instances)
}

pub fn update_instance(paths: &Paths, id: &str, f: impl FnOnce(&mut Instance)) -> Result<()> {
    let mut instances = load_instances(paths);
    let instance = instances
        .iter_mut()
        .find(|i| i.id == id)
        .context("This instance no longer exists.")?;
    f(instance);
    save_instances(paths, &instances)
}

// ---------- Accounts ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub uuid: String,
    pub username: String,
    #[serde(default)]
    pub xuid: String,
    pub ms_refresh_token: String,
    pub mc_access_token: String,
    /// Unix seconds at which the Minecraft access token expires.
    pub mc_expires_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccountStore {
    pub active: Option<String>,
    pub accounts: Vec<Account>,
}

/// What the UI is allowed to see of an account (no tokens).
#[derive(Debug, Clone, Serialize)]
pub struct AccountView {
    pub uuid: String,
    pub username: String,
}

impl From<&Account> for AccountView {
    fn from(account: &Account) -> Self {
        Self {
            uuid: account.uuid.clone(),
            username: account.username.clone(),
        }
    }
}

pub fn load_accounts(paths: &Paths) -> AccountStore {
    read_json(&paths.accounts_file())
}

pub fn save_accounts(paths: &Paths, store: &AccountStore) -> Result<()> {
    write_json(&paths.accounts_file(), store)
}
