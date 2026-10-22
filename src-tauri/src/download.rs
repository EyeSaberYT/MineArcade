use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use futures::stream::{self, StreamExt};
use serde::de::DeserializeOwned;
use sha1::{Digest, Sha1};

const CONCURRENCY: usize = 16;
const RETRIES: u32 = 3;

#[derive(Debug, Clone)]
pub struct Download {
    pub url: String,
    pub path: PathBuf,
    pub sha1: Option<String>,
    pub size: Option<u64>,
    /// Only meaningful on Unix, where runtime binaries need the executable bit.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub executable: bool,
}

pub fn sha1_hex(data: &[u8]) -> String {
    hex::encode(Sha1::digest(data))
}

pub fn verify_sha1(data: &[u8], expected: Option<&str>, what: &str) -> Result<()> {
    if let Some(expected) = expected {
        if !sha1_hex(data).eq_ignore_ascii_case(expected) {
            bail!("checksum mismatch for {what}");
        }
    }
    Ok(())
}

pub async fn fetch_bytes(http: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let resp = http
        .get(url)
        .send()
        .await
        .with_context(|| format!("could not reach {url}"))?;
    let status = resp.status();
    if !status.is_success() {
        bail!("{url} returned {status}");
    }
    Ok(resp.bytes().await?.to_vec())
}

pub async fn fetch_json<T: DeserializeOwned>(http: &reqwest::Client, url: &str) -> Result<T> {
    let bytes = fetch_bytes(http, url).await?;
    serde_json::from_slice(&bytes).with_context(|| format!("unexpected response from {url}"))
}

/// Writes through a temporary file so an interrupted write never leaves a truncated file behind.
pub async fn write_file(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = temp_path(path);
    tokio::fs::write(&tmp, data)
        .await
        .with_context(|| format!("could not write {}", path.display()))?;
    tokio::fs::rename(&tmp, path)
        .await
        .with_context(|| format!("could not write {}", path.display()))?;
    Ok(())
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

/// A file counts as present when it exists with the expected size. Checksums are verified
/// when a file is downloaded, so launches don't have to re-hash thousands of assets.
async fn is_present(d: &Download) -> bool {
    match tokio::fs::metadata(&d.path).await {
        Ok(meta) if meta.is_file() => d.size.map_or(true, |size| meta.len() == size),
        _ => false,
    }
}

async fn download_one(http: &reqwest::Client, d: &Download) -> Result<()> {
    let mut last_error = None;
    for attempt in 0..RETRIES {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(400 * u64::from(attempt))).await;
        }
        let result = fetch_bytes(http, &d.url).await.and_then(|bytes| {
            verify_sha1(&bytes, d.sha1.as_deref(), &d.url)?;
            Ok(bytes)
        });
        match result {
            Ok(bytes) => {
                write_file(&d.path, &bytes).await?;
                set_executable(d);
                return Ok(());
            }
            Err(e) => last_error = Some(e),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("download of {} failed", d.url)))
}

#[cfg(unix)]
fn set_executable(d: &Download) {
    if d.executable {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&d.path, std::fs::Permissions::from_mode(0o755));
    }
}

#[cfg(not(unix))]
fn set_executable(_d: &Download) {}

/// Downloads every missing file in parallel. `on_progress(done, total)` counts only files
/// that actually had to be downloaded.
pub async fn download_all<F>(http: &reqwest::Client, items: Vec<Download>, on_progress: F) -> Result<()>
where
    F: Fn(usize, usize) + Sync,
{
    let mut seen = HashSet::new();
    let mut missing = Vec::new();
    for d in items {
        if !seen.insert(d.path.clone()) {
            continue;
        }
        if !is_present(&d).await {
            missing.push(d);
        }
    }

    let total = missing.len();
    on_progress(0, total);
    if total == 0 {
        return Ok(());
    }

    let step = (total / 100).max(1);
    let done = AtomicUsize::new(0);
    let done = &done;
    let on_progress = &on_progress;

    // Owned items keep the stream's future free of higher-ranked lifetimes, which tauri
    // commands need in order to be `Send`.
    let results: Vec<Result<()>> = stream::iter(missing)
        .map(move |d: Download| async move {
            let result = download_one(http, &d).await;
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n % step == 0 || n == total {
                on_progress(n, total);
            }
            result
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;

    let errors: Vec<anyhow::Error> = results.into_iter().filter_map(Result::err).collect();
    if let Some(first) = errors.first() {
        bail!(
            "{} file(s) failed to download. Check your internet connection and try again. First error: {first:#}",
            errors.len()
        );
    }
    Ok(())
}
