//! Minecraft account authentication.
//!
//! Orbit Launcher only supports real Microsoft accounts, signed in through the
//! Microsoft/Xbox Live chain with the device code flow (see [`microsoft`]).

pub mod microsoft;

use serde::{Deserialize, Serialize};

use super::paths::{read_json, write_private_json, AppPaths};
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

/// Tokens are renewed this many seconds before they expire, as the Modrinth App
/// does, so a launch that spends a while downloading still starts with a valid one.
const EXPIRY_MARGIN_SECS: i64 = 5 * 60;

impl Account {
    pub fn is_expired(&self) -> bool {
        chrono::Utc::now().timestamp() >= self.expires_at - EXPIRY_MARGIN_SECS
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
        // Holds refresh tokens: other users of the machine must not be able to read it.
        write_private_json(&paths.accounts_file(), self)
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

    /// Replace a stored account, but never bring back one that was signed out
    /// meanwhile. Returns whether it was still there.
    pub fn update(&mut self, account: Account) -> bool {
        match self.accounts.iter_mut().find(|a| a.uuid == account.uuid) {
            Some(existing) => {
                *existing = account;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod store_tests {
    use super::*;

    #[test]
    fn update_does_not_bring_back_a_signed_out_account() {
        let account = Account {
            kind: AccountKind::Microsoft,
            username: "Steve".into(),
            uuid: "abc".into(),
            access_token: "old".into(),
            expires_at: 0,
            refresh_token: None,
            skin_url: None,
        };
        let mut store = AccountStore::default();
        assert!(!store.update(account.clone()));
        assert!(store.accounts.is_empty());

        store.upsert(account.clone());
        assert!(store.update(Account { access_token: "new".into(), ..account }));
        assert_eq!(store.accounts[0].access_token, "new");
    }
}
