//! Java runtime discovery and automatic installation.
//!
//! Detection order (first hit wins):
//!   1. an explicit path from settings/instance config
//!   2. a launcher-managed runtime under `<root>/java`
//!   3. `JAVA_HOME`
//!   4. `java` on `PATH`
//!   5. well-known install locations per OS
//!
//! Installation uses Mojang's public runtime manifest, which is exactly what the
//! official launcher downloads, so we get a Java build Mojang has validated
//! against Minecraft.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::download::{download_many, get_json, DownloadSpec, Progress, ProgressSink};
use crate::core::paths::AppPaths;
use crate::error::{LauncherError, Result};

const RUNTIME_MANIFEST_URL: &str =
    "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";

/// A usable Java installation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaRuntime {
    /// Path to the `java` executable.
    pub path: String,
    /// Full version string reported by the JVM, e.g. `17.0.9`.
    pub version: String,
    /// Major version, e.g. `17`.
    pub major: u32,
    /// `managed`, `JAVA_HOME`, `PATH`, `common` or `explicit`.
    pub source: String,
    /// Vendor string when we can determine it.
    pub vendor: String,
}

impl JavaRuntime {
    pub fn as_path(&self) -> PathBuf {
        PathBuf::from(&self.path)
    }
}

fn java_binary_name() -> &'static str {
    if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }
}

/// Platform key used by Mojang's runtime manifest.
pub fn runtime_platform() -> Option<&'static str> {
    if cfg!(target_os = "windows") {
        if cfg!(target_arch = "x86_64") {
            Some("windows-x64")
        } else {
            Some("windows-x86")
        }
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            Some("mac-os-arm64")
        } else {
            Some("mac-os")
        }
    } else if cfg!(target_os = "linux") {
        Some("linux")
    } else {
        None
    }
}

/// Map a required Java major version to Mojang's runtime component name.
pub fn component_for_major(required: u32) -> &'static str {
    match required {
        0..=8 => "jre-legacy",
        9..=16 => "java-runtime-alpha",
        17..=20 => "java-runtime-gamma",
        21..=24 => "java-runtime-delta",
        // Minecraft 26.x declares `java-runtime-epsilon` (Java 25) and its
        // version.json passes `--sun-misc-unsafe-memory-access`, which only
        // JDK 24+ accepts — anything older dies before the game starts.
        _ => "java-runtime-epsilon",
    }
}

/// Read the major Java version requested by a `version.json`.
///
/// `javaVersion.majorVersion` is authoritative, so the larger of it and the
/// component's known version wins. The component table does double duty: it
/// serves manifests that predate `majorVersion`, and it translates component
/// names to versions when scanning Mojang's runtime manifest (which publishes
/// no version numbers at all).
pub fn required_major(component: &str, fallback: u32) -> u32 {
    let known = match component {
        "jre-legacy" => 8,
        "java-runtime-alpha" => 16,
        "java-runtime-beta" | "java-runtime-gamma" | "java-runtime-gamma-snapshot" => 17,
        "java-runtime-delta" => 21,
        "java-runtime-epsilon" => 25,
        _ => 0,
    };
    known.max(fallback)
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// Run `<java> -version` and parse the result.
pub async fn probe(path: &Path, source: &str) -> Option<JavaRuntime> {
    if !path.exists() {
        return None;
    }

    let output = tokio::process::Command::new(path)
        .arg("-version")
        .output()
        .await
        .ok()?;

    // `java -version` writes to stderr.
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let (version, major) = parse_version(&text)?;
    let vendor = parse_vendor(&text);

    Some(JavaRuntime {
        path: path.to_string_lossy().to_string(),
        version,
        major,
        source: source.to_string(),
        vendor,
    })
}

/// Extract `(full_version, major)` from `java -version` output.
fn parse_version(text: &str) -> Option<(String, u32)> {
    // Look for: version "17.0.9" or openjdk version "1.8.0_392"
    let key = "version \"";
    let start = text.find(key)? + key.len();
    let end = text[start..].find('"')? + start;
    let full = text[start..end].to_string();

    let parts: Vec<&str> = full.split(['.', '_', '-']).collect();
    let major = match parts.first() {
        Some(&"1") => parts.get(1).and_then(|p| p.parse().ok())?,
        Some(first) => first.parse().ok()?,
        None => return None,
    };

    Some((full, major))
}

fn parse_vendor(text: &str) -> String {
    if text.contains("Temurin") {
        "Eclipse Temurin".into()
    } else if text.contains("Zulu") {
        "Azul Zulu".into()
    } else if text.contains("GraalVM") {
        "GraalVM".into()
    } else if text.contains("OpenJ9") {
        "IBM Semeru".into()
    } else if text.contains("Microsoft") {
        "Microsoft OpenJDK".into()
    } else if text.contains("OpenJDK") {
        "OpenJDK".into()
    } else if text.contains("Java(TM)") {
        "Oracle".into()
    } else {
        "Unknown".into()
    }
}

/// Candidate `java` executables that ship with a managed runtime.
fn managed_candidates(paths: &AppPaths) -> Vec<(PathBuf, &'static str)> {
    let mut out = vec![];
    if let Ok(entries) = std::fs::read_dir(paths.java_dir()) {
        for entry in entries.flatten() {
            let bin = entry.path().join("bin").join(java_binary_name());
            out.push((bin, "managed"));
        }
    }
    out
}

/// Candidate locations where a system Java may live.
fn common_candidates() -> Vec<(PathBuf, &'static str)> {
    let mut out = vec![];
    let bin = java_binary_name();

    if let Ok(java_home) = std::env::var("JAVA_HOME") {
        out.push((PathBuf::from(java_home).join("bin").join(bin), "JAVA_HOME"));
    }

    if cfg!(windows) {
        for root in [
            r"C:\Program Files\Java",
            r"C:\Program Files\Eclipse Adoptium",
            r"C:\Program Files\Microsoft",
            r"C:\Program Files\Zulu",
            r"C:\Program Files (x86)\Java",
        ] {
            if let Ok(entries) = std::fs::read_dir(root) {
                for entry in entries.flatten() {
                    out.push((entry.path().join("bin").join(bin), "common"));
                }
            }
        }
        // The official launcher ships its own runtimes.
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            let runtimes = PathBuf::from(local)
                .join("Packages")
                .join("Microsoft.4297127D64EC6_8wekyb3d8bbwe")
                .join("LocalCache")
                .join("Local")
                .join("runtime");
            if let Ok(entries) = std::fs::read_dir(runtimes) {
                for entry in entries.flatten() {
                    for inner in std::fs::read_dir(entry.path()).into_iter().flatten().flatten() {
                        out.push((
                            inner.path().join("bin").join(bin),
                            "common",
                        ));
                    }
                }
            }
        }
    } else if cfg!(target_os = "macos") {
        for root in ["/Library/Java/JavaVirtualMachines", "/System/Volumes/Data/Library/Java/JavaVirtualMachines"] {
            if let Ok(entries) = std::fs::read_dir(root) {
                for entry in entries.flatten() {
                    out.push((
                        entry
                            .path()
                            .join("Contents")
                            .join("Home")
                            .join("bin")
                            .join(bin),
                        "common",
                    ));
                }
            }
        }
        out.push((
            PathBuf::from("/usr/local/opt/openjdk/bin").join(bin),
            "common",
        ));
    } else {
        for root in ["/usr/lib/jvm", "/usr/java", "/opt/java"] {
            if let Ok(entries) = std::fs::read_dir(root) {
                for entry in entries.flatten() {
                    out.push((entry.path().join("bin").join(bin), "common"));
                }
            }
        }
        out.push((PathBuf::from("/usr/bin").join(bin), "common"));
    }

    out
}

/// Find a Java runtime that satisfies `required`, preferring `explicit_path`.
pub async fn detect(
    paths: &AppPaths,
    explicit_path: Option<&str>,
    required: u32,
) -> Option<JavaRuntime> {
    let mut candidates: Vec<(PathBuf, &'static str)> = vec![];

    if let Some(explicit) = explicit_path.filter(|p| !p.trim().is_empty()) {
        candidates.push((PathBuf::from(explicit), "explicit"));
    }
    candidates.extend(managed_candidates(paths));
    candidates.extend(common_candidates());

    // Probes are cheap; run them concurrently and pick the best match.
    let mut found: Vec<JavaRuntime> = vec![];
    let mut join = vec![];
    for (path, source) in candidates {
        join.push(async move { probe(&path, source).await });
    }
    for result in futures::future::join_all(join).await {
        if let Some(runtime) = result {
            found.push(runtime);
        }
    }

    // Deduplicate by path.
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found.dedup_by(|a, b| a.path == b.path);

    // Prefer an exact-or-newer match, then the lowest version that satisfies.
    let mut matching: Vec<JavaRuntime> = found.iter().filter(|r| r.major >= required).cloned().collect();
    matching.sort_by_key(|r| r.major);
    if let Some(best) = matching.into_iter().next() {
        return Some(best);
    }

    // Fall back to whatever we found so the user gets a comprehensible error.
    found.into_iter().max_by_key(|r| r.major)
}

/// Enumerate every Java runtime we can find (for the settings UI).
pub async fn list(paths: &AppPaths) -> Vec<JavaRuntime> {
    let mut candidates = managed_candidates(paths);
    candidates.extend(common_candidates());

    let mut found: Vec<JavaRuntime> = vec![];
    let probes = candidates
        .into_iter()
        .map(|(path, source)| async move { probe(&path, source).await });
    for runtime in futures::future::join_all(probes).await.into_iter().flatten() {
        found.push(runtime);
    }

    found.sort_by(|a, b| a.path.cmp(&b.path));
    found.dedup_by(|a, b| a.path == b.path);
    found.sort_by_key(|r| r.major);
    found
}

// ---------------------------------------------------------------------------
// Installation
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RuntimeManifest {
    #[serde(flatten)]
    platforms: std::collections::HashMap<String, std::collections::HashMap<String, Vec<RuntimeRelease>>>,
}

#[derive(Deserialize)]
struct RuntimeRelease {
    manifest: RuntimeFileRef,
}

#[derive(Deserialize)]
struct RuntimeFileRef {
    url: String,
    #[serde(default)]
    sha1: String,
    #[serde(default)]
    size: u64,
}

#[derive(Deserialize, Default)]
struct RuntimeFiles {
    #[serde(default)]
    files: std::collections::HashMap<String, RuntimeFile>,
}

#[derive(Deserialize)]
struct RuntimeFile {
    #[serde(rename = "type")]
    file_type: String,
    #[serde(default)]
    downloads: Option<RuntimeDownloads>,
    /// Present in Mojang's manifest; we extract every file regardless.
    #[serde(default)]
    #[allow(dead_code)]
    executable: bool,
    #[serde(default)]
    target: Option<String>,
}

#[derive(Deserialize, Default)]
struct RuntimeDownloads {
    #[serde(default)]
    raw: Option<RuntimeFileRef>,
}

/// Download and install the Mojang runtime component that satisfies `required`.
///
/// Returns the installed runtime, ready to be passed to the launcher.
pub async fn install(
    http: &reqwest::Client,
    paths: &AppPaths,
    required: u32,
    sink: ProgressSink,
) -> Result<JavaRuntime> {
    let platform = runtime_platform().ok_or_else(|| {
        LauncherError::Unsupported("automatic Java installation on this platform".into())
    })?;

    let manifest: RuntimeManifest = get_json(http, RUNTIME_MANIFEST_URL).await?;
    let platform_components = manifest
        .platforms
        .get(platform)
        .ok_or_else(|| LauncherError::msg(format!("No Java runtimes available for {platform}")))?;

    // Prefer the component mapped to the required version, but never accept one
    // that is actually too old — a too-old JDK rejects Minecraft's JVM flags and
    // dies with a cryptic "Unrecognized option" before the game starts.
    let preferred = component_for_major(required);
    let preferred_ok = platform_components.contains_key(preferred)
        && required_major(preferred, 0) >= required;

    let component = if preferred_ok {
        preferred
    } else {
        let mut names: Vec<(&str, u32)> = platform_components
            .keys()
            .map(|name| (name.as_str(), required_major(name, 0)))
            .collect();
        names.sort_by_key(|(_, major)| *major);
        names
            .into_iter()
            .rev()
            .find(|(_, major)| *major >= required)
            .map(|(name, _)| name)
            .ok_or_else(|| {
                LauncherError::msg(format!(
                    "Mojang doesn't publish a Java {required} runtime for this platform yet. \
                     Install a JDK {required} yourself and point MineArcade at it in Settings."
                ))
            })?
    };

    let release = platform_components
        .get(component)
        .and_then(|list| list.first())
        .ok_or_else(|| LauncherError::msg(format!("No {component} release for {platform}")))?;

    sink(Progress::new(
        "java",
        format!("Downloading Java {required} ({component})…"),
    ));

    let files: RuntimeFiles = get_json(http, &release.manifest.url).await?;
    let dest_root = paths.java_dir().join(component);

    let mut specs = vec![];
    for (relative, file) in &files.files {
        let target = dest_root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        match file.file_type.as_str() {
            "directory" => {
                std::fs::create_dir_all(&target).map_err(|e| {
                    LauncherError::msg(format!(
                        "Couldn't create the folder {}: {e}",
                        target.display()
                    ))
                })?;
            }
            "file" => {
                if let Some(raw) = file.downloads.as_ref().and_then(|d| d.raw.as_ref()) {
                    // Skip the `target` indirection (used for symlinks); we
                    // simply materialise a real file instead.
                    let _ = &file.target;
                    specs.push(
                        DownloadSpec::new(raw.url.clone(), &target)
                            .with_sha1(raw.sha1.clone())
                            .with_size(raw.size)
                            .with_label(relative.clone()),
                    );
                }
            }
            _ => {}
        }
    }

    download_many(
        http.clone(),
        specs,
        8,
        "java",
        sink.clone(),
    )
    .await
    .map_err(|e| LauncherError::msg(format!("While installing {component}: {e}")))?;

    // Ensure the launcher binary is executable on unix.
    let bin = dest_root.join("bin").join(java_binary_name());
    if !bin.exists() {
        return Err(LauncherError::msg(format!(
            "Java was installed but {} is missing.",
            bin.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755));
    }

    sink(Progress {
        stage: "java".into(),
        label: "Java installed".into(),
        current: 1,
        total: 1,
        files_done: 1,
        files_total: 1,
        finished: true,
    });

    probe(&bin, "managed")
        .await
        .ok_or_else(|| LauncherError::msg("The downloaded Java runtime didn't respond."))
}
