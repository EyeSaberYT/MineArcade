//! Minecraft skin lookup and upload, straight against Mojang's public API.
//!
//! Two halves: [`fetch_view`] reads the profile endpoint to report the skin
//! and cape an account is actually wearing, and [`upload`] sends a local PNG
//! to the skin endpoint. Nothing here touches the account store — callers in
//! `crate::commands` own that — so the module stays testable on its own.

use reqwest::multipart;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use tokio::fs;

use super::auth::microsoft::normalize_texture_url;
use crate::error::{LauncherError, Result};

/// The account's profile: where the active skin and cape are listed.
const PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";
/// Multipart upload target for a new skin (`file` + `variant` form fields).
const SKIN_UPLOAD_URL: &str = "https://api.minecraftservices.com/minecraft/profile/skins";

/// Which model the skin is drawn for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkinVariant {
    /// Wide (4px) arms — the classic Steve/Alex-independent model.
    #[default]
    Classic,
    /// Thin (3px) arms.
    Slim,
}

impl SkinVariant {
    /// The spelling the upload endpoint expects.
    pub fn as_mojang(&self) -> &'static str {
        match self {
            Self::Classic => "CLASSIC",
            Self::Slim => "SLIM",
        }
    }

    /// Parse the profile endpoint's spelling (case-insensitive).
    pub fn parse(value: &str) -> Self {
        match value.to_ascii_uppercase().as_str() {
            "SLIM" => Self::Slim,
            _ => Self::Classic,
        }
    }
}

/// What an account is currently wearing, ready to hand to the UI.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkinView {
    /// Texture URL for the active skin, already normalised for the webview.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skin_url: Option<String>,
    /// Model the texture is drawn for, when the profile says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<SkinVariant>,
    /// Texture URL for the active cape, if one is equipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cape_url: Option<String>,
}

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

/// One `skins[]` entry as Mojang returns it.
#[derive(Deserialize)]
struct ProfileSkin {
    state: String,
    url: String,
    /// `CLASSIC` or `SLIM`.
    #[serde(default)]
    variant: Option<String>,
}

/// One `capes[]` entry as Mojang returns it.
#[derive(Deserialize)]
struct ProfileCape {
    state: String,
    url: String,
}

/// The slice of `/minecraft/profile` this launcher cares about.
#[derive(Deserialize)]
struct Profile {
    #[serde(default)]
    skins: Vec<ProfileSkin>,
    #[serde(default)]
    capes: Vec<ProfileCape>,
}

impl Profile {
    fn into_view(self) -> SkinView {
        // Prefer the entry marked ACTIVE; fall back to the first one, since a
        // profile with skins but none active still tells us the model in use.
        let skin = self
            .skins
            .iter()
            .find(|s| s.state == "ACTIVE")
            .or_else(|| self.skins.first());
        let cape = self.capes.iter().find(|c| c.state == "ACTIVE");

        SkinView {
            skin_url: skin.map(|s| normalize_texture_url(&s.url)),
            variant: skin.and_then(|s| s.variant.as_deref()).map(SkinVariant::parse),
            cape_url: cape.map(|c| normalize_texture_url(&c.url)),
        }
    }
}

/// The account's current skin and cape.
///
/// A failure here is not fatal — the caller keeps whatever URL it already
/// stored — so the error is only surfaced when it wants to show it.
pub async fn fetch_view(http: &reqwest::Client, mc_access_token: &str) -> Result<SkinView> {
    let res = http
        .get(PROFILE_URL)
        .bearer_auth(mc_access_token)
        .send()
        .await?;

    if !res.status().is_success() {
        return Err(LauncherError::Auth(
            "Minecraft services wouldn't hand over the profile.".into(),
        ));
    }

    let profile: Profile = res.json().await?;
    Ok(profile.into_view())
}

// ---------------------------------------------------------------------------
// Upload
// ---------------------------------------------------------------------------

/// Dimensions of a Minecraft skin, read straight from the PNG header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkinDimensions {
    pub width: u32,
    pub height: u32,
}

/// Read width/height from a PNG's IHDR chunk.
///
/// Only the header is inspected, so this costs a few dozen bytes rather than
/// a full decode, and a file that isn't a PNG at all fails with a message a
/// user can act on instead of an image-library panic.
pub fn skin_dimensions(bytes: &[u8]) -> Result<SkinDimensions> {
    // 8-byte signature, then a length (4) and the `IHDR` marker (4); width and
    // height follow as big-endian u32s.
    const PNG_SIGNATURE: [u8; 8] = [137, b'P', b'N', b'G', 13, 10, 26, 10];
    if bytes.len() < 24 || bytes[..8] != PNG_SIGNATURE || &bytes[12..16] != b"IHDR" {
        return Err(LauncherError::InvalidData(
            "That file isn't a PNG image.".into(),
        ));
    }

    let read = |at: usize| -> u32 {
        (u32::from(bytes[at]) << 24)
            | (u32::from(bytes[at + 1]) << 16)
            | (u32::from(bytes[at + 2]) << 8)
            | u32::from(bytes[at + 3])
    };
    let dimensions = SkinDimensions {
        width: read(16),
        height: read(20),
    };

    // Mojang accepts the current 64×64 sheet and the legacy 64×32 one.
    match (dimensions.width, dimensions.height) {
        (64, 64) | (64, 32) => Ok(dimensions),
        (w, h) => Err(LauncherError::InvalidData(format!(
            "Skins must be 64×64 or 64×32 pixels — that image is {w}×{h}."
        ))),
    }
}

/// Upload a local PNG as the account's new skin.
///
/// The file is validated before it leaves the machine, so an obviously wrong
/// image costs a round trip rather than a rejection from Mojang. On success
/// the texture URL from the response is returned, which is what the caller
/// should store and display from now on.
pub async fn upload(
    http: &reqwest::Client,
    mc_access_token: &str,
    png_path: &Path,
    variant: SkinVariant,
) -> Result<Option<String>> {
    let bytes = fs::read(png_path).await?;
    skin_dimensions(&bytes)?;

    let file = multipart::Part::bytes(bytes)
        .file_name("skin.png")
        .mime_str("image/png")?;
    let form = multipart::Form::new()
        .part("file", file)
        .text("variant", variant.as_mojang().to_string());

    let res = http
        .post(SKIN_UPLOAD_URL)
        .bearer_auth(mc_access_token)
        .multipart(form)
        .send()
        .await?;

    match res.status().as_u16() {
        200 => {
            let profile: Profile = res.json().await?;
            Ok(profile.into_view().skin_url)
        }
        // Mojang rate-limits this endpoint; the message has to say so,
        // otherwise "upload failed" reads like a broken file.
        429 => Err(LauncherError::RateLimited(
            "Minecraft is rate-limiting skin changes. Try again in a minute.".into(),
        )),
        401 | 403 => Err(LauncherError::Auth(
            "Your Microsoft session expired — sign in again, then retry.".into(),
        )),
        status => {
            let body = res.text().await.unwrap_or_default();
            let message = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|value| {
                    value
                        .get("errorMessage")
                        .or_else(|| value.get("error"))
                        .and_then(|field| field.as_str())
                        .map(str::to_owned)
                })
                .unwrap_or(body);
            Err(LauncherError::Minecraft(if message.trim().is_empty() {
                format!("Minecraft rejected the skin (HTTP {status}).")
            } else {
                message
            }))
        }
    }
}

/// Cache-busting token for a texture URL.
///
/// `textures.minecraft.net` answers from cache, so after an upload the new
/// skin can still be served from the old bytes until the URL changes. A
/// query string is enough to make the webview ask again.
pub fn skin_preview_url(skin_url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(skin_url.as_bytes());
    let token: String = hasher
        .finalize()
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect();

    if skin_url.contains('?') {
        format!("{skin_url}&t={token}")
    } else {
        format!("{skin_url}?t={token}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal but valid PNG header: signature, IHDR length, marker, size.
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = vec![137, b'P', b'N', b'G', 13, 10, 26, 10];
        bytes.extend_from_slice(&13u32.to_be_bytes()); // IHDR length
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]); // depth/colour/compression
        bytes
    }

    #[test]
    fn accepts_both_skin_sheets() {
        assert_eq!(
            skin_dimensions(&png_header(64, 64)).unwrap(),
            SkinDimensions { width: 64, height: 64 }
        );
        assert_eq!(
            skin_dimensions(&png_header(64, 32)).unwrap(),
            SkinDimensions { width: 64, height: 32 }
        );
    }

    #[test]
    fn refuses_other_sizes() {
        let error = skin_dimensions(&png_header(128, 128)).unwrap_err();
        assert!(error.to_string().contains("128×128"), "{error}");
    }

    #[test]
    fn refuses_files_that_are_not_pngs() {
        assert!(skin_dimensions(b"not a png at all, just text really").is_err());
        assert!(skin_dimensions(&[]).is_err());
    }

    #[test]
    fn variants_match_the_wire_format() {
        assert_eq!(SkinVariant::Classic.as_mojang(), "CLASSIC");
        assert_eq!(SkinVariant::Slim.as_mojang(), "SLIM");
        assert_eq!(SkinVariant::parse("slim"), SkinVariant::Slim);
        assert_eq!(SkinVariant::parse("CLASSIC"), SkinVariant::Classic);
        assert_eq!(SkinVariant::parse("something-else"), SkinVariant::Classic);
    }

    #[test]
    fn preview_urls_bust_the_cache_without_breaking_query_strings() {
        let plain = skin_preview_url("https://textures.minecraft.net/texture/abc");
        assert!(plain.starts_with("https://textures.minecraft.net/texture/abc?t="));

        let queried = skin_preview_url("https://example.test/skin.png?v=1");
        assert!(queried.starts_with("https://example.test/skin.png?v=1&t="));
    }
}
