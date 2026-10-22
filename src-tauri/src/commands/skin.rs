//! Skin viewing and upload commands.
//!
//! The dialog calls `skin_current` for who is signed in and what they are
//! wearing, and `skin_upload` once a file has been picked. Both go through
//! [`active_account`], so a token that expired since sign-in is refreshed
//! before either call does anything, and the result is written back to the
//! account store so the avatar in the menu picks the new skin up too.

use std::path::Path;

use serde::Serialize;
use tauri::State;

use crate::{
    commands::auth::{active_account, PublicAccount},
    core::{
        auth::Account,
        skin::{self, SkinVariant, SkinView},
    },
    error::{LauncherError, Result},
    state::AppState,
};

/// Everything the skin dialog needs, in one round trip.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkinCurrentResponse {
    /// `null` when nobody is signed in — the dialog then shows an empty state
    /// rather than an error.
    pub account: Option<PublicAccount>,
    /// What the account is wearing; empty when it could not be determined.
    pub skin: SkinView,
}

/// Tokens are stripped here exactly as they are for every other command.
fn public_of(account: &Account) -> PublicAccount {
    PublicAccount {
        uuid: account.uuid.clone(),
        username: account.username.clone(),
        kind: account.kind,
        skin_url: account.skin_url.clone(),
        active: true,
    }
}

/// Read the live skin and cape, falling back to the stored URL when the
/// profile endpoint cannot answer (expired session, network).
async fn current_skin(state: &AppState, account: &Account) -> SkinView {
    if account.access_token.is_empty() {
        return SkinView {
            skin_url: account.skin_url.clone(),
            variant: None,
            cape_url: None,
        };
    }

    match skin::fetch_view(&state.http, &account.access_token).await {
        Ok(view) => view,
        Err(cause) => {
            log::debug!("skin lookup failed, using the stored url: {cause}");
            SkinView {
                skin_url: account.skin_url.clone(),
                variant: None,
                cape_url: None,
            }
        }
    }
}

#[tauri::command]
pub async fn skin_current(state: State<'_, AppState>) -> Result<SkinCurrentResponse> {
    let signed_in = {
        let store = state.accounts.read().await;
        store.active_account().is_some()
    };
    if !signed_in {
        return Ok(SkinCurrentResponse {
            account: None,
            skin: SkinView::default(),
        });
    }

    let account = active_account(&state).await?;
    let skin = current_skin(&state, &account).await;
    Ok(SkinCurrentResponse {
        account: Some(public_of(&account)),
        skin,
    })
}

#[tauri::command]
pub async fn skin_upload(
    state: State<'_, AppState>,
    path: String,
    variant: SkinVariant,
) -> Result<SkinCurrentResponse> {
    let account = active_account(&state).await.map_err(|_| {
        LauncherError::Auth("Sign in with a Microsoft account to change your skin.".into())
    })?;
    // Validated (PNG + 64×64 / 64×32) and uploaded by the core module; a bad
    // file fails here with a message that says what was wrong with it.
    let uploaded =
        skin::upload(&state.http, &account.access_token, Path::new(&path), variant).await?;

    // Persist, so the menu avatar shows the new skin even if the profile
    // endpoint is slow or unreachable the next time it is asked.
    let saved = {
        let mut store = state.accounts.write().await;
        if let Some(entry) = store.accounts.iter_mut().find(|a| a.uuid == account.uuid) {
            entry.skin_url = match uploaded {
                Some(url) => Some(url),
                // Mojang did not hand back a URL: keep the old one but bust
                // its cache so the change is visible on the next read.
                None => entry
                    .skin_url
                    .clone()
                    .map(|url| skin::skin_preview_url(&url)),
            };
        }
        store.save(&state.paths)?;
        store.active_account().cloned()
    };

    let account = saved.unwrap_or(account);
    let skin = current_skin(&state, &account).await;
    Ok(SkinCurrentResponse {
        account: Some(public_of(&account)),
        skin,
    })
}
