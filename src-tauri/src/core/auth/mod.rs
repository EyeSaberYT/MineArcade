//! Minecraft account authentication.
//!
//! MineArcade only supports real Microsoft accounts, signed in through the
//! Microsoft/Xbox Live chain with the device code flow (see [`microsoft`]).

pub mod microsoft;

use serde::{Deserialize, Serialize};

use super::paths::{read_json, write_json, AppPaths};
use crate::error::Result;

/// How an account authenticates. Kept as an enum so the on-disk format and the
/// UI payload can grow without a migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountKind {
    Microsoft,
}

/// A signed-in Minecraft account.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub kind: AccountKind,
    pub username: String,
    /// Dashless Minecraft UUID.
    pub uuid: String,
    pub access_token: String,
    /// Unix timestamp (seconds) at which `access_token` expires.
    pub expires_at: i64,
    pub refresh_token: Option<String>,
    /// Player skin texture url, when available.
    #[serde(default)]
    pub skin_url: Option<String>,
}

impl Account {
    pub fn is_expired(&self) -> bool {
        chrono::Utc::now().timestamp() >= self.expires_at - 60
    }

    /// The `user_type` value passed to the game.
    pub fn user_type(&self) -> &'static str {
        match self.kind {
            AccountKind::Microsoft => "msa",
        }
    }
}

/// Persistent list of accounts plus the currently selected one.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStore {
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub active: Option<String>,
}

impl AccountStore {
    pub fn load(paths: &AppPaths) -> Result<Self> {
        Ok(read_json(&paths.accounts_file())?.unwrap_or_default())
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        write_json(&paths.accounts_file(), self)
    }

    pub fn active_account(&self) -> Option<&Account> {
        let active = self.active.as_ref()?;
        self.accounts.iter().find(|a| a.uuid == *active)
    }

    pub fn upsert(&mut self, account: Account) {
        if let Some(existing) = self.accounts.iter_mut().find(|a| a.uuid == account.uuid) {
            *existing = account;
        } else {
            self.accounts.push(account);
        }
    }
}
