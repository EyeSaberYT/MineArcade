//! Friends, through Orbit Client's cosmetics server (the same server and sign-in Orbit Client
//! uses, so friends, requests and messages are shared between the launcher and the game).
//!
//! The server is found through DNS, like Orbit Client does: an SRV record gives its host and port.
//! Signing in works like joining a Minecraft server: a one-time challenge, Mojang's `join` with the
//! account's token, then the server checks it with Mojang and hands out its own token. No Microsoft
//! token ever reaches the friends server.

use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::Mutex;

use super::auth::Account;
use crate::error::{LauncherError, Result};

const SRV_NAME: &str = "_eclipse-cosmetics._tcp.minearcade.org";
const RESOLVE_EVERY: Duration = Duration::from_secs(30 * 60);
const MOJANG_JOIN: &str = "https://sessionserver.mojang.com/session/minecraft/join";

/// Where the friends server is, and the sign-in for one account.
#[derive(Default)]
pub struct Social {
    server: Mutex<Option<(String, Instant)>>,
    /// (account uuid, server, token)
    token: Mutex<Option<(String, String, String)>>,
}

impl Social {
    /// The server's base URL (`http://host:port`), looked up again every half hour.
    async fn server(&self, http: &reqwest::Client) -> Result<String> {
        let mut cached = self.server.lock().await;
        if let Some((url, at)) = cached.as_ref() {
            if at.elapsed() < RESOLVE_EVERY {
                return Ok(url.clone());
            }
        }
        let url = resolve(http).await?;
        *cached = Some((url.clone(), Instant::now()));
        Ok(url)
    }

    async fn token(&self, http: &reqwest::Client, server: &str, account: &Account) -> Result<String> {
        let mut token = self.token.lock().await;
        if let Some((uuid, for_server, value)) = token.as_ref() {
            if uuid == &account.uuid && for_server == server {
                return Ok(value.clone());
            }
        }
        let value = sign_in(http, server, account).await?;
        *token = Some((account.uuid.clone(), server.to_string(), value.clone()));
        Ok(value)
    }

    /// POST `path` as `account`; signs in again once when the server says the token expired.
    pub async fn call(&self, http: &reqwest::Client, account: &Account, path: &str, body: Option<String>) -> Result<Value> {
        let server = self.server(http).await?;
        for attempt in 0..2 {
            let token = self.token(http, &server, account).await?;
            let mut request = http
                .post(format!("{server}{path}"))
                .timeout(Duration::from_secs(10))
                .bearer_auth(&token)
                .header("Content-Type", "text/plain; charset=utf-8");
            if let Some(body) = &body {
                request = request.body(body.clone());
            }
            let response = request.send().await.map_err(|_| unreachable_server())?;
            let status = response.status();
            if status.as_u16() == 401 && attempt == 0 {
                *self.token.lock().await = None;
                continue;
            }
            let text = response.text().await.unwrap_or_default();
            let json: Value = match serde_json::from_str(&text) {
                Ok(json) => json,
                // An old cosmetics server without these routes answers with a 404 page.
                Err(_) if status.as_u16() == 404 => return Err(LauncherError::msg("Orbit's friends server needs an update.")),
                Err(_) => return Err(LauncherError::msg(format!("Friends server error ({status})"))),
            };
            if !status.is_success() {
                let message = json.get("error").and_then(Value::as_str).unwrap_or("Friends server error");
                return Err(LauncherError::msg(message.to_string()));
            }
            return Ok(json);
        }
        Err(LauncherError::msg("Couldn't sign in to Orbit's friends server."))
    }
}

fn unreachable_server() -> LauncherError {
    LauncherError::msg("Can't reach Orbit's friends server.")
}

/// Look up the SRV record over DNS-over-HTTPS (Google, then Cloudflare): "priority weight port host".
async fn resolve(http: &reqwest::Client) -> Result<String> {
    let resolvers = [
        format!("https://dns.google/resolve?name={SRV_NAME}&type=SRV"),
        format!("https://cloudflare-dns.com/dns-query?name={SRV_NAME}&type=SRV"),
    ];
    for url in resolvers {
        let Ok(response) = http.get(&url).header("Accept", "application/dns-json").timeout(Duration::from_secs(8)).send().await else {
            continue;
        };
        let Ok(json) = response.json::<Value>().await else { continue };
        if let Some(server) = parse_srv(&json) {
            return Ok(server);
        }
    }
    Err(unreachable_server())
}

fn parse_srv(json: &Value) -> Option<String> {
    let answers = json.get("Answer")?.as_array()?;
    for answer in answers {
        // Type 33 is SRV.
        if answer.get("type").and_then(Value::as_u64) != Some(33) {
            continue;
        }
        let data = answer.get("data")?.as_str()?;
        let parts: Vec<&str> = data.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }
        let port: u16 = parts[2].parse().ok()?;
        let host = parts[3].trim_end_matches('.');
        if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
            continue;
        }
        return Some(format!("http://{host}:{port}"));
    }
    None
}

async fn sign_in(http: &reqwest::Client, server: &str, account: &Account) -> Result<String> {
    let challenge: Value = http
        .get(format!("{server}/auth/challenge"))
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .map_err(|_| unreachable_server())?
        .json()
        .await
        .map_err(|_| LauncherError::msg("Friends server error (sign-in)."))?;
    let server_id = challenge
        .get("serverId")
        .and_then(Value::as_str)
        .ok_or_else(|| LauncherError::msg("Friends server error (sign-in)."))?
        .to_string();

    // Tell Mojang this account is joining "server_id"; the friends server then asks Mojang.
    let joined = http
        .post(MOJANG_JOIN)
        .timeout(Duration::from_secs(10))
        .json(&serde_json::json!({
            "accessToken": account.access_token,
            "selectedProfile": account.uuid.replace('-', ""),
            "serverId": server_id,
        }))
        .send()
        .await
        .map_err(|_| LauncherError::msg("Couldn't reach Mojang to sign in to Orbit's friends server."))?;
    if !joined.status().is_success() {
        return Err(LauncherError::Auth("Mojang refused the sign-in. Sign in to your account again.".into()));
    }

    let verified = http
        .post(format!("{server}/auth/verify"))
        .query(&[("name", account.username.as_str()), ("serverId", server_id.as_str())])
        .timeout(Duration::from_secs(12))
        .send()
        .await
        .map_err(|_| unreachable_server())?;
    if !verified.status().is_success() {
        return Err(LauncherError::msg(format!("Orbit's friends server refused the sign-in ({}).", verified.status())));
    }
    let json: Value = verified.json().await.map_err(|_| LauncherError::msg("Friends server error (sign-in)."))?;
    json.get("token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| LauncherError::msg("Friends server error (sign-in)."))
}

#[cfg(test)]
mod tests {
    use super::parse_srv;

    #[test]
    fn reads_the_srv_record() {
        let json = serde_json::json!({"Answer": [
            {"name": "_eclipse-cosmetics._tcp.minearcade.org.", "type": 33, "data": "0 5 25598 panel.minearcade.org."}
        ]});
        assert_eq!(parse_srv(&json).as_deref(), Some("http://panel.minearcade.org:25598"));
        // A CNAME first, then the SRV.
        let json = serde_json::json!({"Answer": [
            {"type": 5, "data": "elsewhere.example."},
            {"type": 33, "data": "0 5 8787 cosmetics.example.org."}
        ]});
        assert_eq!(parse_srv(&json).as_deref(), Some("http://cosmetics.example.org:8787"));
        assert_eq!(parse_srv(&serde_json::json!({"Status": 3})), None);
        assert_eq!(parse_srv(&serde_json::json!({"Answer": [{"type": 33, "data": "0 5 80 evil/host."}]})), None);
    }
}
