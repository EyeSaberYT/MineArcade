use anyhow::{bail, Context, Result};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::download::{fetch_bytes, write_file};
use crate::state::AppState;
use crate::versions::VersionJson;

const META: &str = "https://meta.fabricmc.net/v2";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoaderVersion {
    pub version: String,
    pub stable: bool,
}

#[derive(Deserialize)]
struct LoaderEntry {
    loader: LoaderVersion,
}

/// Fabric loader versions for a Minecraft version, newest first. Empty when unsupported.
pub async fn loader_versions(http: &reqwest::Client, game_version: &str) -> Result<Vec<LoaderVersion>> {
    let url = format!("{META}/versions/loader/{game_version}");
    let resp = http
        .get(&url)
        .send()
        .await
        .context("Could not reach Fabric's servers")?;
    // Fabric answers 400 with an empty list for versions it doesn't support.
    if resp.status() == StatusCode::BAD_REQUEST || resp.status() == StatusCode::NOT_FOUND {
        return Ok(Vec::new());
    }
    if !resp.status().is_success() {
        bail!("Fabric's servers returned {}", resp.status());
    }
    let entries: Vec<LoaderEntry> = resp
        .json()
        .await
        .context("Fabric returned an unexpected version list")?;
    Ok(entries.into_iter().map(|e| e.loader).collect())
}

pub async fn latest_loader(http: &reqwest::Client, game_version: &str) -> Result<String> {
    let versions = loader_versions(http, game_version).await?;
    versions
        .iter()
        .find(|v| v.stable)
        .or(versions.first())
        .map(|v| v.version.clone())
        .with_context(|| format!("Fabric does not support Minecraft {game_version}."))
}

/// Downloads Fabric's launcher profile, a version JSON that inherits from vanilla.
pub async fn ensure_profile(state: &AppState, game_version: &str, loader_version: &str) -> Result<VersionJson> {
    let id = format!("fabric-loader-{loader_version}-{game_version}");
    let path = state.paths.version_json(&id);
    if let Ok(bytes) = tokio::fs::read(&path).await {
        if let Ok(version) = serde_json::from_slice::<VersionJson>(&bytes) {
            return Ok(version);
        }
    }

    let url = format!("{META}/versions/loader/{game_version}/{loader_version}/profile/json");
    let bytes = fetch_bytes(&state.http, &url).await.with_context(|| {
        format!("Fabric {loader_version} is not available for Minecraft {game_version}")
    })?;
    let version: VersionJson =
        serde_json::from_slice(&bytes).context("Fabric returned an invalid profile")?;
    write_file(&path, &bytes).await?;
    Ok(version)
}
