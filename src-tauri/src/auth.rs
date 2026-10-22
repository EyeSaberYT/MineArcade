//! Microsoft -> Xbox Live -> XSTS -> Minecraft sign-in, using the OAuth device code flow.

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::{self, now_secs, Account};
use crate::state::AppState;

pub const NO_CLIENT_ID: &str =
    "Add your Azure application (client) ID in Settings before signing in.";

const SCOPE: &str = "XboxLive.signin offline_access";
const DEVICE_CODE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const XBL_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

/// Refresh the Minecraft token when it has less than this many seconds left.
const REFRESH_MARGIN_SECS: u64 = 600;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Deserialize)]
struct OAuthError {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Deserialize)]
pub struct MsToken {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

enum Poll {
    Pending,
    SlowDown,
    Done(MsToken),
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct XboxResponse {
    token: String,
    display_claims: DisplayClaims,
}

#[derive(Deserialize)]
struct DisplayClaims {
    xui: Vec<XboxUser>,
}

#[derive(Deserialize)]
struct XboxUser {
    uhs: String,
    #[serde(default)]
    xid: Option<String>,
}

#[derive(Deserialize)]
struct McLogin {
    access_token: String,
    expires_in: u64,
}

#[derive(Deserialize)]
struct Profile {
    id: String,
    name: String,
}

/// Microsoft's error descriptions start with a useful line and continue with trace ids.
fn oauth_message(body: &str) -> String {
    match serde_json::from_str::<OAuthError>(body) {
        Ok(e) => e
            .error_description
            .unwrap_or(e.error)
            .lines()
            .next()
            .unwrap_or_default()
            .to_string(),
        Err(_) => "Microsoft returned an unexpected response.".to_string(),
    }
}

pub async fn start_device_flow(http: &reqwest::Client, client_id: &str) -> Result<DeviceCode> {
    let resp = http
        .post(DEVICE_CODE_URL)
        .form(&[("client_id", client_id), ("scope", SCOPE)])
        .send()
        .await
        .context("Could not reach Microsoft")?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        bail!("Microsoft sign-in failed: {}", oauth_message(&body));
    }
    serde_json::from_str(&body).context("Microsoft returned an unexpected device code response")
}

async fn poll_device_flow(http: &reqwest::Client, client_id: &str, device_code: &str) -> Result<Poll> {
    let sent = http
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("client_id", client_id),
            ("device_code", device_code),
        ])
        .send()
        .await;
    // A network hiccup while polling shouldn't abort the sign-in; try again next interval.
    let Ok(resp) = sent else { return Ok(Poll::Pending) };

    let status = resp.status();
    let body = resp.text().await?;
    if status.is_success() {
        let token = serde_json::from_str(&body).context("Microsoft returned an unexpected token")?;
        return Ok(Poll::Done(token));
    }
    let error: OAuthError = serde_json::from_str(&body)
        .map_err(|_| anyhow!("Microsoft sign-in failed ({status})"))?;
    match error.error.as_str() {
        "authorization_pending" => Ok(Poll::Pending),
        "slow_down" => Ok(Poll::SlowDown),
        "authorization_declined" => bail!("Sign-in was declined."),
        "expired_token" | "code_expired" => bail!("The sign-in code expired. Please try again."),
        _ => bail!("Microsoft sign-in failed: {}", oauth_message(&body)),
    }
}

async fn refresh_ms(http: &reqwest::Client, client_id: &str, refresh_token: &str) -> Result<MsToken> {
    let resp = http
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_token),
            ("scope", SCOPE),
        ])
        .send()
        .await
        .context("Could not reach Microsoft")?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        bail!(
            "Your Microsoft session has expired, please sign in again ({})",
            oauth_message(&body)
        );
    }
    serde_json::from_str(&body).context("Microsoft returned an unexpected token")
}

async fn xbox_live(http: &reqwest::Client, ms_access_token: &str) -> Result<XboxResponse> {
    let resp = http
        .post(XBL_URL)
        .header("Accept", "application/json")
        .json(&json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={ms_access_token}"),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        }))
        .send()
        .await
        .context("Could not reach Xbox Live")?;
    if !resp.status().is_success() {
        bail!("Xbox Live sign-in failed ({})", resp.status());
    }
    resp.json().await.context("Xbox Live returned an unexpected response")
}

async fn xsts_authorize(http: &reqwest::Client, xbl_token: &str) -> Result<XboxResponse> {
    let resp = http
        .post(XSTS_URL)
        .header("Accept", "application/json")
        .json(&json!({
            "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl_token] },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT",
        }))
        .send()
        .await
        .context("Could not reach Xbox Live")?;
    let status = resp.status();
    if status == StatusCode::UNAUTHORIZED {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        let message = match body["XErr"].as_u64() {
            Some(2148916233) => "This Microsoft account has no Xbox profile yet. Sign in once at minecraft.net to create one, then try again.",
            Some(2148916235) => "Xbox Live is not available in your country.",
            Some(2148916236) | Some(2148916237) => "This account needs adult verification on the Xbox website.",
            Some(2148916238) => "This is a child account. An adult has to add it to a Microsoft family group first.",
            _ => "Xbox Live refused to authorize this account.",
        };
        bail!("{message}");
    }
    if !status.is_success() {
        bail!("Xbox Live authorization failed ({status})");
    }
    resp.json().await.context("Xbox Live returned an unexpected response")
}

async fn minecraft_login(http: &reqwest::Client, uhs: &str, xsts_token: &str) -> Result<McLogin> {
    let resp = http
        .post(MC_LOGIN_URL)
        .json(&json!({ "identityToken": format!("XBL3.0 x={uhs};{xsts_token}") }))
        .send()
        .await
        .context("Could not reach Minecraft services")?;
    let status = resp.status();
    if status == StatusCode::FORBIDDEN || status == StatusCode::UNAUTHORIZED {
        bail!(
            "Minecraft services rejected the sign-in. Mojang has to approve your Azure client ID for \
             Minecraft first (https://aka.ms/mce-reviewappid)."
        );
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        bail!("Too many sign-in attempts. Wait a minute and try again.");
    }
    if !status.is_success() {
        bail!("Minecraft sign-in failed ({status})");
    }
    resp.json().await.context("Minecraft services returned an unexpected response")
}

async fn fetch_profile(http: &reqwest::Client, mc_token: &str) -> Result<Profile> {
    let resp = http
        .get(PROFILE_URL)
        .bearer_auth(mc_token)
        .send()
        .await
        .context("Could not reach Minecraft services")?;
    let status = resp.status();
    if status == StatusCode::NOT_FOUND {
        bail!("This Microsoft account doesn't own Minecraft: Java Edition, or hasn't picked a profile name yet.");
    }
    if !status.is_success() {
        bail!("Could not load your Minecraft profile ({status})");
    }
    resp.json().await.context("Minecraft services returned an unexpected profile")
}

/// Reads a claim from a JWT payload without verifying it (only used for the xuid).
fn jwt_claim(token: &str, claim: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    match &value[claim] {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

async fn complete_login(http: &reqwest::Client, ms: MsToken, previous_refresh: Option<&str>) -> Result<Account> {
    let xbl = xbox_live(http, &ms.access_token).await?;
    let xsts = xsts_authorize(http, &xbl.token).await?;
    let user = xsts
        .display_claims
        .xui
        .first()
        .context("Xbox Live did not return a user hash")?;
    let mc = minecraft_login(http, &user.uhs, &xsts.token).await?;
    let profile = fetch_profile(http, &mc.access_token).await?;

    let xuid = user
        .xid
        .clone()
        .or_else(|| jwt_claim(&mc.access_token, "xuid"))
        .unwrap_or_default();
    let refresh_token = ms
        .refresh_token
        .or_else(|| previous_refresh.map(str::to_string))
        .context("Microsoft did not return a refresh token")?;

    Ok(Account {
        uuid: profile.id,
        username: profile.name,
        xuid,
        ms_refresh_token: refresh_token,
        mc_access_token: mc.access_token,
        mc_expires_at: now_secs() + mc.expires_in,
    })
}

/// Polls Microsoft until the user finishes signing in, then stores and activates the account.
pub async fn wait_for_login(state: &AppState, device_code: &str, interval: u64, expires_in: u64) -> Result<Account> {
    let client_id = config::load_settings(&state.paths).effective_client_id();
    let deadline = Instant::now() + Duration::from_secs(expires_in.max(60));
    let mut interval = interval.max(1);

    loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;

        let still_active = {
            let current = state.login_device_code.lock().unwrap();
            current.as_deref() == Some(device_code)
        };
        if !still_active {
            bail!("cancelled");
        }
        if Instant::now() >= deadline {
            bail!("The sign-in code expired. Please try again.");
        }

        match poll_device_flow(&state.http, &client_id, device_code).await? {
            Poll::Pending => {}
            Poll::SlowDown => interval += 5,
            Poll::Done(token) => {
                let account = complete_login(&state.http, token, None).await?;

                let _lock = state.accounts_lock.lock().await;
                let mut store = config::load_accounts(&state.paths);
                store.accounts.retain(|a| a.uuid != account.uuid);
                store.accounts.push(account.clone());
                store.active = Some(account.uuid.clone());
                config::save_accounts(&state.paths, &store)?;

                *state.login_device_code.lock().unwrap() = None;
                return Ok(account);
            }
        }
    }
}

/// Returns the active account, refreshing its Minecraft token first when it is about to expire.
pub async fn active_account(state: &AppState, client_id: &str) -> Result<Account> {
    let _lock = state.accounts_lock.lock().await;
    let mut store = config::load_accounts(&state.paths);
    let active = store
        .active
        .clone()
        .context("Sign in with your Microsoft account first.")?;
    let index = store
        .accounts
        .iter()
        .position(|a| a.uuid == active)
        .context("Sign in with your Microsoft account first.")?;

    let account = &store.accounts[index];
    if account.mc_expires_at > now_secs() + REFRESH_MARGIN_SECS {
        return Ok(account.clone());
    }
    if client_id.is_empty() {
        bail!(NO_CLIENT_ID);
    }

    let ms = refresh_ms(&state.http, client_id, &account.ms_refresh_token).await?;
    let previous_refresh = account.ms_refresh_token.clone();
    let refreshed = complete_login(&state.http, ms, Some(&previous_refresh)).await?;
    store.accounts[index] = refreshed.clone();
    config::save_accounts(&state.paths, &store)?;
    Ok(refreshed)
}
