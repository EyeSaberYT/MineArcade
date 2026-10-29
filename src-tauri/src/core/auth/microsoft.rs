//! Microsoft → Xbox Live → Minecraft authentication chain.
//!
//! We use the **OAuth 2.0 device code flow**, which is the friendliest option
//! for a desktop launcher: the user is shown a short code, opens
//! `microsoft.com/link` in any browser, and the launcher polls until the sign-in
//! completes. No embedded browser, no client secret, no loopback server.
//!
//! The chain is:
//!
//! ```text
//! Microsoft identity (device code)
//!   └─ Xbox Live user token (XBL)
//!        └─ XSTS token (relying party: rp://api.minecraftservices.com/)
//!             └─ Minecraft services token
//!                  └─ Minecraft profile (uuid + name)
//! ```

use serde::Deserialize;

use super::{Account, AccountKind};
use crate::error::{LauncherError, Result};

/// Microsoft identity authority used for sign-in.
///
/// This must be `consumers`. The `XboxLive.signin` scope only exists for
/// personal Microsoft accounts; requesting it against a directory id or
/// `common` fails with AADSTS650053 ("scope ... doesn't exist on the resource
/// Microsoft Graph"). The app registration must therefore be configured to
/// allow personal Microsoft accounts (see the README), and users must sign in
/// with the personal account that owns Minecraft.
const TENANT: &str = "consumers";

fn device_code_url() -> String {
    format!("https://login.microsoftonline.com/{TENANT}/oauth2/v2.0/devicecode")
}

fn token_url() -> String {
    format!("https://login.microsoftonline.com/{TENANT}/oauth2/v2.0/token")
}
const XBL_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MC_PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";
const MC_ENTITLEMENTS_URL: &str = "https://api.minecraftservices.com/entitlements/mcstore";

const SCOPE: &str = "XboxLive.signin offline_access";

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCodeInfo {
    pub user_code: String,
    pub verification_uri: String,
    pub message: String,
    pub expires_in: u64,
}

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    message: String,
    expires_in: u64,
    #[serde(default = "default_interval")]
    interval: u64,
}

fn default_interval() -> u64 {
    5
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default = "default_expires")]
    expires_in: i64,
}

fn default_expires() -> i64 {
    3600
}

#[derive(Deserialize)]
struct OAuthError {
    error: String,
    #[serde(default)]
    error_description: String,
    #[serde(default)]
    xerr: Option<u64>,
}

#[derive(Deserialize)]
struct XblResponse {
    #[serde(rename = "Token")]
    token: String,
    #[serde(rename = "DisplayClaims")]
    display_claims: DisplayClaims,
}

#[derive(Deserialize)]
struct DisplayClaims {
    xui: Vec<Xui>,
}

#[derive(Deserialize)]
struct Xui {
    uhs: String,
}

#[derive(Deserialize)]
struct McTokenResponse {
    access_token: String,
    #[serde(default = "default_expires")]
    expires_in: i64,
}

#[derive(Deserialize)]
struct McProfile {
    id: String,
    name: String,
    #[serde(default)]
    skins: Vec<McSkin>,
}

#[derive(Deserialize)]
struct McSkin {
    state: String,
    url: String,
}

#[derive(Deserialize)]
struct McEntitlements {
    #[serde(default)]
    items: Vec<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Flow steps
// ---------------------------------------------------------------------------

/// Step 1: ask Microsoft for a device + user code.
pub async fn request_device_code(
    http: &reqwest::Client,
    client_id: &str,
) -> Result<(DeviceCodeInfo, String, u64)> {
    let res = http
        .post(device_code_url())
        .form(&[("client_id", client_id), ("scope", SCOPE)])
        .send()
        .await?;

    if !res.status().is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(LauncherError::Auth(format!(
            "Microsoft rejected the device code request. Check that your client id is correct. ({body})"
        )));
    }

    let device: DeviceCodeResponse = res.json().await?;
    let info = DeviceCodeInfo {
        user_code: device.user_code.clone(),
        verification_uri: device.verification_uri.clone(),
        message: if device.message.is_empty() {
            "To sign in, open the link and enter the code below.".into()
        } else {
            device.message.clone()
        },
        expires_in: device.expires_in,
    };
    Ok((info, device.device_code, device.interval.max(1)))
}

/// Step 2: poll the token endpoint until the user finishes signing in.
pub async fn poll_for_token(
    http: &reqwest::Client,
    client_id: &str,
    device_code: &str,
    interval: u64,
    expires_in: u64,
) -> Result<(String, Option<String>, i64)> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(expires_in);
    let mut delay = interval;

    loop {
        if std::time::Instant::now() > deadline {
            return Err(LauncherError::Auth(
                "The sign-in code expired before it was used. Please try again.".into(),
            ));
        }
        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;

        let res = http
            .post(token_url())
            .form(&[
                ("client_id", client_id),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code),
            ])
            .send()
            .await?;

        if res.status().is_success() {
            let token: TokenResponse = res.json().await?;
            return Ok((token.access_token, token.refresh_token, token.expires_in));
        }

        // Polling errors are expected while the user is still authenticating.
        let err: OAuthError = res.json().await.unwrap_or(OAuthError {
            error: "unknown_error".into(),
            error_description: String::new(),
            xerr: None,
        });

        match err.error.as_str() {
            "authorization_pending" => continue,
            "slow_down" => {
                delay += 5;
                continue;
            }
            "expired_token" | "bad_verification_code" => {
                return Err(LauncherError::Auth(
                    "The sign-in code expired. Please try again.".into(),
                ))
            }
            "authorization_declined" => {
                return Err(LauncherError::Auth("Sign-in was declined.".into()))
            }
            other => {
                return Err(LauncherError::Auth(format!(
                    "Microsoft sign-in failed ({other}): {}",
                    err.error_description
                )))
            }
        }
    }
}

/// How many times a refresh is attempted when the connection itself fails
/// (as the Modrinth App does): a dropped Wi-Fi packet is not a lost sign-in.
const REFRESH_ATTEMPTS: u32 = 4;

/// Exchange a refresh token for a fresh Microsoft access token.
///
/// Only `invalid_grant` means the sign-in is really over (revoked, expired after
/// 90 days unused, password changed); every other failure is reported as it is,
/// so a temporary outage doesn't look like being signed out.
pub async fn refresh_token(
    http: &reqwest::Client,
    client_id: &str,
    refresh_token: &str,
) -> Result<(String, Option<String>, i64)> {
    let mut attempt = 1;
    let res = loop {
        let sent = http
            .post(token_url())
            .form(&[
                ("client_id", client_id),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("scope", SCOPE),
            ])
            .send()
            .await;
        match sent {
            Ok(res) => break res,
            Err(error) if (error.is_connect() || error.is_timeout()) && attempt < REFRESH_ATTEMPTS => {
                log::warn!("Microsoft token refresh attempt {attempt} couldn't connect: {error}");
                tokio::time::sleep(std::time::Duration::from_millis(500 * u64::from(attempt))).await;
                attempt += 1;
            }
            Err(error) => return Err(error.into()),
        }
    };

    let status = res.status();
    if !status.is_success() {
        let body = res.text().await.unwrap_or_default();
        let err: Option<OAuthError> = serde_json::from_str(&body).ok();
        // The description starts with an AADSTS code that says exactly why; log
        // it in full, show its first line.
        log::warn!("Microsoft token refresh failed (HTTP {status}): {body}");
        let detail = err
            .as_ref()
            .map(|e| e.error_description.lines().next().unwrap_or_default().trim().to_string())
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(LauncherError::Auth(match err.as_ref().map(|e| e.error.as_str()) {
            Some("invalid_grant") => format!("Microsoft ended this sign-in, so please sign in again. ({detail})"),
            Some(code) => format!("Microsoft couldn't renew the sign-in ({code}): {detail}"),
            None => format!("Microsoft couldn't renew the sign-in: {detail}"),
        }));
    }

    let token: TokenResponse = res.json().await?;
    Ok((token.access_token, token.refresh_token, token.expires_in))
}

/// Step 3: trade the Microsoft access token for an XSTS token and a Minecraft
/// services token.
pub async fn xbox_chain(
    http: &reqwest::Client,
    ms_access_token: &str,
) -> Result<(String, i64)> {
    // 3a. Xbox Live user token.
    let xbl_res = http
        .post(XBL_URL)
        .json(&serde_json::json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={ms_access_token}"),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        }))
        .send()
        .await?;

    // Xbox Live returns a non-2xx body with a different shape on failure, so
    // check the status before decoding (otherwise it surfaces as a confusing
    // "error decoding response body" network error).
    if !xbl_res.status().is_success() {
        let status = xbl_res.status();
        let body = xbl_res.text().await.unwrap_or_default();
        return Err(LauncherError::Auth(format!(
            "Xbox Live rejected the Microsoft sign-in (HTTP {status}). {body}"
        )));
    }
    let xbl: XblResponse = xbl_res.json().await?;

    let uhs = xbl
        .display_claims
        .xui
        .first()
        .map(|x| x.uhs.clone())
        .ok_or_else(|| LauncherError::Auth("Xbox Live returned no user hash.".into()))?;

    // 3b. XSTS token.
    let xsts_res = http
        .post(XSTS_URL)
        .json(&serde_json::json!({
            "Properties": {
                "SandboxId": "RETAIL",
                "UserTokens": [xbl.token],
            },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT",
        }))
        .send()
        .await?;

    if !xsts_res.status().is_success() {
        let err: OAuthError = xsts_res.json().await.unwrap_or(OAuthError {
            error: "xsts_error".into(),
            error_description: String::new(),
            xerr: None,
        });
        let message = match err.xerr {
            Some(2148916233) => {
                "This Microsoft account has no Xbox profile yet. Sign in at xbox.com once, then try again."
            }
            Some(2148916235) => "Xbox Live is not available in your region.",
            Some(2148916236) | Some(2148916237) => {
                "This account needs adult verification before it can play Minecraft."
            }
            Some(2148916238) => {
                "This is a child account. Ask a family organiser to add it to a Microsoft Family group."
            }
            _ => "Xbox Live rejected the sign-in. Please try again later.",
        };
        return Err(LauncherError::Auth(message.into()));
    }

    let xsts: XblResponse = xsts_res.json().await?;

    // 3c. Minecraft services token.
    let mc_res = http
        .post(MC_LOGIN_URL)
        .json(&serde_json::json!({
            "identityToken": format!("XBL3.0 x={uhs};{}", xsts.token),
        }))
        .send()
        .await?;

    // Minecraft services returns a non-2xx body with a different shape on
    // failure, so check the status before decoding. This is also where a brand
    // new Azure app registration is rejected until Mojang allow-lists it.
    if !mc_res.status().is_success() {
        let status = mc_res.status();
        let body = mc_res.text().await.unwrap_or_default();

        if body.contains("Invalid app registration") {
            return Err(LauncherError::Auth(
                "This build's Azure app registration hasn't been allow-listed by Mojang, so \
                 the final Minecraft login step is blocked (HTTP 403, \"Invalid app \
                 registration\"). The client id needs approval at \
                 https://aka.ms/mce-reviewappid."
                    .into(),
            ));
        }
        return Err(LauncherError::Auth(format!(
            "Minecraft services rejected the login (HTTP {status}). {body}"
        )));
    }
    let mc: McTokenResponse = mc_res.json().await?;

    Ok((mc.access_token, mc.expires_in))
}

/// Step 4: fetch the player's Minecraft profile (uuid + username).
pub async fn fetch_profile(
    http: &reqwest::Client,
    mc_access_token: &str,
) -> Result<(String, String, Option<String>)> {
    let res = http
        .get(MC_PROFILE_URL)
        .bearer_auth(mc_access_token)
        .send()
        .await?;

    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(LauncherError::Auth(
            "This Microsoft account doesn't own Minecraft: Java Edition.".into(),
        ));
    }
    if !res.status().is_success() {
        return Err(LauncherError::Auth(
            "Minecraft services rejected the login. Please try again.".into(),
        ));
    }

    let profile: McProfile = res.json().await?;
    let skin_url = profile
        .skins
        .iter()
        .find(|s| s.state == "ACTIVE")
        .map(|s| s.url.as_str())
        .or_else(|| profile.skins.first().map(|s| s.url.as_str()))
        .map(normalize_texture_url);
    Ok((dashify_uuid(&profile.id), profile.name, skin_url))
}

/// Mojang hands back `http://textures.minecraft.net/…`, but the app's CSP only
/// allows `https://` for that host, so normalise before the url is stored or
/// handed to the webview.
pub(crate) fn normalize_texture_url(url: &str) -> String {
    match url.strip_prefix("http://textures.minecraft.net") {
        Some(rest) => format!("https://textures.minecraft.net{rest}"),
        None => url.to_string(),
    }
}

/// Look up a stored account's skin, back-filling it when missing.
///
/// Accounts saved by builds that didn't record a texture url (or signed in
/// while the profile still reported none) keep showing a placeholder until this
/// runs, so the accounts command fills the gap on read. Deliberately best
/// effort: a missing skin never breaks sign-in, and it is refreshed again on
/// the next token refresh.
pub async fn skin_for_account(
    http: &reqwest::Client,
    account: &Account,
) -> Result<Option<String>> {
    if let Some(skin) = account.skin_url.as_ref() {
        return Ok(Some(skin.clone()));
    }
    // An expired token would only produce a 401; the next refresh fills it in.
    if account.is_expired() {
        return Ok(None);
    }

    let (_, _, skin) = fetch_profile(http, &account.access_token).await?;
    Ok(skin)
}

/// Confirm the account actually owns the game.
pub async fn check_entitlements(http: &reqwest::Client, mc_access_token: &str) -> Result<()> {
    let res = http
        .get(MC_ENTITLEMENTS_URL)
        .bearer_auth(mc_access_token)
        .send()
        .await?;

    if !res.status().is_success() {
        // Not fatal: some accounts legitimately return 204/404 here.
        return Ok(());
    }

    if let Ok(entitlements) = res.json::<McEntitlements>().await {
        if entitlements.items.is_empty() {
            return Err(LauncherError::Auth(
                "This Microsoft account doesn't own Minecraft: Java Edition.".into(),
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// High level helpers
// ---------------------------------------------------------------------------

/// Complete device-code sign-in, returning a ready-to-use [`Account`].
///
/// `on_code` is invoked once the user code is available so the UI can display
/// it while we poll.
pub async fn login<F>(
    http: &reqwest::Client,
    client_id: &str,
    on_code: F,
) -> Result<Account>
where
    F: FnOnce(&DeviceCodeInfo),
{
    let (info, device_code, interval) = request_device_code(http, client_id).await?;
    on_code(&info);

    let (ms_token, refresh, _) =
        poll_for_token(http, client_id, &device_code, interval, info.expires_in).await?;

    let (mc_token, expires_in) = xbox_chain(http, &ms_token).await?;
    check_entitlements(http, &mc_token).await.ok();
    let (uuid, name, skin_url) = fetch_profile(http, &mc_token).await?;

    Ok(Account {
        kind: AccountKind::Microsoft,
        username: name,
        uuid,
        access_token: mc_token,
        expires_at: chrono::Utc::now().timestamp() + expires_in,
        refresh_token: refresh,
        skin_url,
    })
}

/// Re-authenticate a stored Microsoft account using its refresh token.
pub async fn refresh_account(
    http: &reqwest::Client,
    client_id: &str,
    account: &Account,
) -> Result<Account> {
    let refresh = account
        .refresh_token
        .clone()
        .ok_or_else(|| LauncherError::Auth("No refresh token stored for this account.".into()))?;

    let (ms_token, new_refresh, _) = refresh_token(http, client_id, &refresh).await?;
    let (mc_token, expires_in) = xbox_chain(http, &ms_token).await?;
    let (uuid, name, skin_url) = fetch_profile(http, &mc_token).await?;

    Ok(Account {
        kind: AccountKind::Microsoft,
        username: name,
        uuid,
        access_token: mc_token,
        expires_at: chrono::Utc::now().timestamp() + expires_in,
        refresh_token: new_refresh.or(account.refresh_token.clone()),
        skin_url: skin_url.or_else(|| account.skin_url.clone()),
    })
}

/// Insert dashes into a 32-char dashless UUID.
pub fn dashify_uuid(raw: &str) -> String {
    if raw.len() != 32 {
        return raw.to_string();
    }
    format!(
        "{}-{}-{}-{}-{}",
        &raw[0..8],
        &raw[8..12],
        &raw[12..16],
        &raw[16..20],
        &raw[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_urls_are_upgraded_to_https() {
        // The webview's CSP only allows https for this host.
        assert_eq!(
            normalize_texture_url("http://textures.minecraft.net/texture/abc"),
            "https://textures.minecraft.net/texture/abc"
        );
        assert_eq!(
            normalize_texture_url("https://textures.minecraft.net/texture/abc"),
            "https://textures.minecraft.net/texture/abc"
        );
        // Anything else is left exactly as received.
        assert_eq!(
            normalize_texture_url("http://example.test/x"),
            "http://example.test/x"
        );
    }
}
