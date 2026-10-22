//! Downloads the Java runtime Mojang ships for each Minecraft version, so players
//! never have to install Java themselves.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::download::{download_all, fetch_json, Download};
use crate::progress::Progress;
use crate::state::AppState;

const RUNTIME_INDEX: &str = "https://piston-meta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";
const MARKER: &str = ".minearcade-complete";

#[derive(Deserialize)]
struct RuntimeEntry {
    manifest: FileRef,
}

#[derive(Deserialize)]
struct FileRef {
    sha1: String,
    size: u64,
    url: String,
}

#[derive(Deserialize)]
struct RuntimeManifest {
    files: HashMap<String, RuntimeFile>,
}

#[derive(Deserialize)]
struct RuntimeFile {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    executable: bool,
    #[serde(default)]
    downloads: Option<RuntimeDownloads>,
    #[serde(default)]
    #[cfg_attr(not(unix), allow(dead_code))]
    target: Option<String>,
}

#[derive(Deserialize)]
struct RuntimeDownloads {
    raw: FileRef,
}

fn platform() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("windows-x64"),
        ("windows", "x86") => Some("windows-x86"),
        ("windows", "aarch64") => Some("windows-arm64"),
        ("macos", "x86_64") => Some("mac-os"),
        ("macos", "aarch64") => Some("mac-os-arm64"),
        ("linux", "x86_64") => Some("linux"),
        ("linux", "x86") => Some("linux-i386"),
        _ => None,
    }
}

fn java_executable(dir: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        dir.join("jre.bundle/Contents/Home/bin/java")
    } else if cfg!(windows) {
        // javaw has no console window; its output is still captured through pipes.
        dir.join("bin").join("javaw.exe")
    } else {
        dir.join("bin/java")
    }
}

/// Returns the java executable for `component` (for example "java-runtime-delta"),
/// downloading the runtime first if it isn't installed yet.
pub async fn ensure_runtime(state: &AppState, component: &str, progress: &Progress) -> Result<PathBuf> {
    let platform = platform().context(
        "Mojang has no Java runtime for this platform. Set a custom Java path in the instance settings.",
    )?;
    let dir = state.paths.runtimes().join(component).join(platform);
    let java = java_executable(&dir);
    if dir.join(MARKER).exists() && java.exists() {
        return Ok(java);
    }

    progress.stage("java", format!("Downloading Java ({component})"));
    let index: HashMap<String, HashMap<String, Vec<RuntimeEntry>>> =
        fetch_json(&state.http, RUNTIME_INDEX).await?;
    let entry = index
        .get(platform)
        .and_then(|components| components.get(component))
        .and_then(|entries| entries.first())
        .with_context(|| {
            format!("Mojang has no {component} Java runtime for {platform}. Set a custom Java path in the instance settings.")
        })?;
    let manifest: RuntimeManifest = fetch_json(&state.http, &entry.manifest.url).await?;

    let mut downloads = Vec::new();
    for (relative, file) in &manifest.files {
        let target = dir.join(relative);
        match file.kind.as_str() {
            "directory" => tokio::fs::create_dir_all(&target).await?,
            "file" => {
                if let Some(d) = &file.downloads {
                    downloads.push(Download {
                        url: d.raw.url.clone(),
                        path: target,
                        sha1: Some(d.raw.sha1.clone()),
                        size: Some(d.raw.size),
                        executable: file.executable,
                    });
                }
            }
            "link" => {
                #[cfg(unix)]
                if let Some(link_target) = &file.target {
                    if let Some(parent) = target.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    let _ = tokio::fs::remove_file(&target).await;
                    let _ = std::os::unix::fs::symlink(link_target, &target);
                }
            }
            _ => {}
        }
    }

    download_all(&state.http, downloads, |done, total| progress.update(done, total)).await?;
    tokio::fs::write(dir.join(MARKER), &entry.manifest.sha1).await?;
    Ok(java)
}
