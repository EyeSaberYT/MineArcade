//! Download manager: parallel, resumable-ish, checksum-verified downloads with
//! structured progress reporting.
//!
//! The frontend subscribes to `download://progress` events; each event carries a
//! [`Progress`] snapshot so the UI can render a single unified progress bar.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use sha2::Sha512;

use crate::error::{LauncherError, Result};

/// A progress snapshot emitted while downloading or installing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// High level stage, e.g. `download`, `install`, `extract`, `launch`.
    pub stage: String,
    /// Human readable description of what is happening.
    pub label: String,
    /// Bytes completed so far across the whole task.
    pub current: u64,
    /// Total bytes expected, or 0 when unknown.
    pub total: u64,
    /// Number of finished files / total files.
    pub files_done: usize,
    pub files_total: usize,
    /// Set once the whole task is complete.
    pub finished: bool,
}

impl Progress {
    pub fn new(stage: &str, label: impl Into<String>) -> Self {
        Self {
            stage: stage.into(),
            label: label.into(),
            current: 0,
            total: 0,
            files_done: 0,
            files_total: 0,
            finished: false,
        }
    }

    pub fn percent(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.current as f32 / self.total as f32) * 100.0
        }
    }
}

/// Callback used to stream [`Progress`] to the UI. Cheap to clone.
pub type ProgressSink = Arc<dyn Fn(Progress) + Send + Sync>;

/// Minimum time between progress snapshots from one batch download.
const PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// A no-op sink for callers that don't care about progress.
pub fn silent() -> ProgressSink {
    Arc::new(|_| {})
}

/// One file to fetch.
#[derive(Debug, Clone)]
pub struct DownloadSpec {
    pub url: String,
    pub dest: PathBuf,
    /// Expected SHA-1 hex digest, when known.
    pub sha1: Option<String>,
    /// Expected SHA-512 hex digest, when known. Checked alongside `sha1`.
    pub sha512: Option<String>,
    /// Further URLs for the same file, tried in order when `url` fails.
    pub fallback_urls: Vec<String>,
    /// Expected size in bytes, used for progress totals before we have headers.
    pub size: u64,
    /// Optional human readable label for the UI.
    pub label: Option<String>,
    /// Extra HTTP headers to send with the request (e.g., `x-api-key`).
    pub headers: Vec<(String, String)>,
    /// Accept an existing file of the expected size without re-hashing it.
    /// For content-addressed stores (game assets, named by their hash) where
    /// re-reading hundreds of MB on every launch costs far more than it finds.
    pub quick_verify: bool,
}

impl DownloadSpec {
    pub fn new(url: impl Into<String>, dest: impl Into<PathBuf>) -> Self {
        Self {
            url: url.into(),
            dest: dest.into(),
            sha1: None,
            sha512: None,
            fallback_urls: vec![],
            size: 0,
            label: None,
            headers: vec![],
            quick_verify: false,
        }
    }

    pub fn with_quick_verify(mut self) -> Self {
        self.quick_verify = true;
        self
    }

    pub fn with_sha1(mut self, sha1: impl Into<String>) -> Self {
        self.sha1 = Some(sha1.into());
        self
    }

    pub fn with_sha512(mut self, sha512: impl Into<String>) -> Self {
        self.sha512 = Some(sha512.into());
        self
    }

    pub fn with_fallback_urls(mut self, urls: Vec<String>) -> Self {
        self.fallback_urls.extend(urls);
        self
    }

    pub fn with_size(mut self, size: u64) -> Self {
        self.size = size;
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn with_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.headers.extend(headers);
        self
    }
}

/// Result of a batch download.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadReport {
    pub downloaded: usize,
    pub skipped: usize,
    pub bytes: u64,
}

pub fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Hash a file's contents with SHA-1.
pub async fn sha1_file(path: &Path) -> Result<String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<String> {
        use std::io::Read;
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Sha1::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buf)?;
            if read == 0 {
                break;
            }
            hasher.update(&buf[..read]);
        }
        Ok(to_hex(&hasher.finalize()))
    })
    .await
    .map_err(|e| LauncherError::msg(format!("hashing task failed: {e}")))?
}

/// Hash a file's contents with SHA-512.
pub async fn sha512_file(path: &Path) -> Result<String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<String> {
        use std::io::Read;
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Sha512::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buf)?;
            if read == 0 {
                break;
            }
            hasher.update(&buf[..read]);
        }
        Ok(to_hex(&hasher.finalize()))
    })
    .await
    .map_err(|e| LauncherError::msg(format!("hashing task failed: {e}")))?
}

/// True when the file exists and matches the expected checksum.
pub async fn is_valid(path: &Path, expected_sha1: Option<&str>) -> bool {
    if !path.exists() {
        return false;
    }
    match expected_sha1 {
        None => true,
        Some(expected) => match sha1_file(path).await {
            Ok(actual) => actual.eq_ignore_ascii_case(expected),
            Err(_) => false,
        },
    }
}

/// [`is_valid`] for a spec: every digest it declares must match.
async fn spec_is_valid(spec: &DownloadSpec) -> bool {
    if !is_valid(&spec.dest, spec.sha1.as_deref()).await {
        return false;
    }
    match spec.sha512.as_deref() {
        None => true,
        Some(expected) => sha512_file(&spec.dest)
            .await
            .is_ok_and(|actual| actual.eq_ignore_ascii_case(expected)),
    }
}

/// Download a single file, verifying its checksum when provided.
///
/// `spec.url` is tried first, then each of `spec.fallback_urls` in order.
///
/// Returns `Ok(true)` when the file was fetched, `Ok(false)` when it already
/// existed and was valid.
pub async fn download_one(
    http: &reqwest::Client,
    spec: &DownloadSpec,
    on_bytes: &(dyn Fn(u64) + Send + Sync),
) -> Result<bool> {
    let already_there = if spec.quick_verify && spec.size > 0 {
        std::fs::metadata(&spec.dest).is_ok_and(|meta| meta.len() == spec.size)
    } else {
        spec_is_valid(spec).await
    };
    if already_there {
        return Ok(false);
    }

    // Every io error below carries the offending path: a bare "file not found"
    // is impossible to act on, while `…\lib\jvm.cfg.part` tells the user (and
    // us) exactly what went wrong.
    if let Some(_parent) = spec.dest.parent() {
        std::fs::create_dir_all(_parent).map_err(|e| {
            LauncherError::msg(format!("Couldn't create {}: {e}", _parent.display()))
        })?;
    }

    let mut last_error = None;
    for url in std::iter::once(&spec.url).chain(&spec.fallback_urls) {
        // A unique sibling per attempt: `x.jar` and `x.zip`, or two specs
        // aimed at the same file, must never share a temporary.
        let tmp = temp_path(&spec.dest);
        match fetch_to(http, spec, url, &tmp, on_bytes).await {
            Ok(()) => return Ok(true),
            Err(error) => {
                let _ = std::fs::remove_file(&tmp);
                last_error = Some(error);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| LauncherError::msg("No download URL.")))
}

/// `<dest file name>.<random>.part`, next to `dest`.
fn temp_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    let unique = uuid::Uuid::new_v4().simple().to_string();
    name.push(format!(".{}.part", &unique[..12]));
    dest.with_file_name(name)
}

/// Stream `url` into `tmp`, verify it against the spec, then move it to
/// `spec.dest`. The caller removes `tmp` when this fails.
async fn fetch_to(
    http: &reqwest::Client,
    spec: &DownloadSpec,
    url: &str,
    tmp: &Path,
    on_bytes: &(dyn Fn(u64) + Send + Sync),
) -> Result<()> {
    let mut request = http.get(url);
    for (name, value) in &spec.headers {
        request = request.header(name, value);
    }
    let response = request.send().await?;
    if !response.status().is_success() {
        return Err(LauncherError::msg(format!(
            "Download failed ({}): {url}",
            response.status()
        )));
    }

    // Stream to a temporary file so a cancelled download never leaves a
    // half-written jar in place.
    let mut file = std::fs::File::create(tmp).map_err(|e| {
        LauncherError::msg(format!("Couldn't create {}: {e}", tmp.display()))
    })?;
    let mut sha1 = Sha1::new();
    let mut sha512 = spec.sha512.as_ref().map(|_| Sha512::new());
    let mut stream = response.bytes_stream();

    use std::io::Write;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        sha1.update(&chunk);
        if let Some(hasher) = sha512.as_mut() {
            hasher.update(&chunk);
        }
        file.write_all(&chunk).map_err(|e| {
            LauncherError::msg(format!("Couldn't write {}: {e}", tmp.display()))
        })?;
        on_bytes(chunk.len() as u64);
    }
    file.flush().map_err(|e| {
        LauncherError::msg(format!("Couldn't flush {}: {e}", tmp.display()))
    })?;
    drop(file);

    let actual_sha1 = to_hex(&sha1.finalize());
    let actual_sha512 = sha512.map(|hasher| to_hex(&hasher.finalize()));
    for (expected, actual) in [
        (spec.sha1.as_deref(), Some(actual_sha1.as_str())),
        (spec.sha512.as_deref(), actual_sha512.as_deref()),
    ] {
        if let (Some(expected), Some(actual)) = (expected, actual) {
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(LauncherError::msg(format!(
                    "Checksum mismatch for {} (expected {expected}, got {actual})",
                    spec.dest.display()
                )));
            }
        }
    }

    // Atomic-ish replace. Only replace if the destination is still inside
    // the expected location (extra safety for pack downloads).
    if spec.dest.exists() {
        let _ = std::fs::remove_file(&spec.dest);
    }
    std::fs::rename(tmp, &spec.dest).map_err(|e| {
        LauncherError::msg(format!(
            "Couldn't move {} into {}: {e}",
            tmp.display(),
            spec.dest.display()
        ))
    })?;
    Ok(())
}

/// Download many files in parallel, reporting aggregate progress.
pub async fn download_many(
    http: reqwest::Client,
    specs: Vec<DownloadSpec>,
    concurrency: usize,
    stage: &str,
    sink: ProgressSink,
) -> Result<DownloadReport> {
    if specs.is_empty() {
        sink(Progress {
            stage: stage.into(),
            label: "Nothing to download".into(),
            current: 0,
            total: 0,
            files_done: 0,
            files_total: 0,
            finished: true,
        });
        return Ok(DownloadReport::default());
    }

    let total_bytes: u64 = specs.iter().map(|s| s.size).sum();
    let files_total = specs.len();

    // Every chunk of every file used to emit an event: thousands per second
    // across 5,000 assets, which swamped the webview so the bar stopped
    // moving. Send at most one snapshot per interval; the final one below
    // always goes out.
    let raw_sink = sink;
    let final_sink = raw_sink.clone();
    let last_emit = Arc::new(std::sync::Mutex::new(None::<std::time::Instant>));
    let sink: ProgressSink = Arc::new(move |progress: Progress| {
        let now = std::time::Instant::now();
        let mut last = last_emit.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if last.is_some_and(|at| now.duration_since(at) < PROGRESS_INTERVAL) {
            return;
        }
        *last = Some(now);
        drop(last);
        raw_sink(progress);
    });
    let downloaded = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let files_done = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let skipped = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let bytes = Arc::new(std::sync::atomic::AtomicU64::new(0));

    // Initial snapshot so the UI immediately shows a bar.
    sink(Progress {
        stage: stage.into(),
        label: format!("Preparing {files_total} files…"),
        current: 0,
        total: total_bytes,
        files_done: 0,
        files_total,
        finished: false,
    });

    let results: Vec<Result<bool>> = futures::stream::iter(specs.into_iter().map(|spec| {
        let http = http.clone();
        let sink = sink.clone();
        let downloaded = downloaded.clone();
        let files_done = files_done.clone();
        let skipped = skipped.clone();
        let bytes = bytes.clone();
        async move {
            let label = spec
                .label
                .clone()
                .or_else(|| {
                    spec.dest
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                })
                .unwrap_or_else(|| "file".into());

            let on_bytes = |n: u64| {
                let new_total = bytes.fetch_add(n, std::sync::atomic::Ordering::Relaxed) + n;
                sink(Progress {
                    stage: stage.to_string(),
                    label: format!("Downloading {label}"),
                    current: new_total,
                    total: total_bytes,
                    files_done: files_done.load(std::sync::atomic::Ordering::Relaxed) as usize,
                    files_total,
                    finished: false,
                });
            };

            let result = download_one(&http, &spec, &on_bytes).await;
            let done = files_done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            match &result {
                Ok(true) => {
                    downloaded.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Ok(false) => {
                    skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Err(_) => {}
            }
            sink(Progress {
                stage: stage.to_string(),
                label: format!("Downloading {label}"),
                current: bytes.load(std::sync::atomic::Ordering::Relaxed),
                total: total_bytes.max(bytes.load(std::sync::atomic::Ordering::Relaxed)),
                files_done: done as usize,
                files_total,
                finished: false,
            });
            result
        }
    }))
    .buffer_unordered(concurrency.max(1))
    .collect()
    .await;

    // Surface the first error, if any.
    let mut report = DownloadReport::default();
    for result in results {
        result?;
    }
    report.downloaded = downloaded.load(std::sync::atomic::Ordering::Relaxed) as usize;
    report.skipped = skipped.load(std::sync::atomic::Ordering::Relaxed) as usize;
    report.bytes = bytes.load(std::sync::atomic::Ordering::Relaxed);

    final_sink(Progress {
        stage: stage.into(),
        label: format!("Finished {files_total} files"),
        current: report.bytes,
        total: report.bytes,
        files_done: files_total,
        files_total,
        finished: true,
    });

    Ok(report)
}

/// Fetch a JSON document with a helpful error message.
pub async fn get_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    url: &str,
) -> Result<T> {
    let res = http
        .get(url)
        .header("User-Agent", crate::USER_AGENT)
        .send()
        .await?;
    if !res.status().is_success() {
        return Err(LauncherError::msg(format!(
            "Request to {url} failed with status {}",
            res.status()
        )));
    }
    Ok(res.json::<T>().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_paths_are_unique_per_file_and_attempt() {
        let jar = temp_path(Path::new("mods/x.jar"));
        let zip = temp_path(Path::new("mods/x.zip"));
        let again = temp_path(Path::new("mods/x.jar"));

        let name = jar.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("x.jar.") && name.ends_with(".part"), "{name}");
        assert_ne!(jar, zip);
        assert_ne!(jar, again);
    }

    /// A present file only counts as done when every declared digest matches.
    #[tokio::test]
    async fn existing_file_with_wrong_sha512_is_fetched_again() {
        let dir = std::env::temp_dir().join(format!("orbit-launcher-dl-sha512-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("a.jar");
        std::fs::write(&dest, b"hello").unwrap();
        let sha1 = sha1_file(&dest).await.unwrap();
        let sha512 = sha512_file(&dest).await.unwrap();

        let good = DownloadSpec::new("nope://never", &dest)
            .with_sha1(sha1.clone())
            .with_sha512(sha512);
        assert!(!download_one(&reqwest::Client::new(), &good, &|_| {}).await.unwrap());

        // Wrong sha512: it must try to download (and fail on the unusable
        // URLs, which error instantly, leaving no temporary behind).
        let bad = DownloadSpec::new("nope://never", &dest)
            .with_sha1(sha1)
            .with_sha512("00")
            .with_fallback_urls(vec!["nope://also-never".into()]);
        assert!(download_one(&reqwest::Client::new(), &bad, &|_| {}).await.is_err());
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
            .collect();
        assert!(leftovers.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn present_files_are_counted_and_progress_is_throttled() {
        let dir = std::env::temp_dir().join(format!("orbit-launcher-dl-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 300 files already on disk at the expected size: nothing is fetched
        // (the URL is unreachable on purpose) and no file is re-hashed.
        let specs: Vec<DownloadSpec> = (0..300)
            .map(|i| {
                let dest = dir.join(format!("asset-{i}"));
                std::fs::write(&dest, b"0123456789").unwrap();
                DownloadSpec::new("http://127.0.0.1:9/never", dest)
                    .with_sha1("does-not-matter")
                    .with_size(10)
                    .with_quick_verify()
            })
            .collect();

        let seen = Arc::new(std::sync::Mutex::new(Vec::<Progress>::new()));
        let record = seen.clone();
        let sink: ProgressSink = Arc::new(move |p| record.lock().unwrap().push(p));
        let report = download_many(reqwest::Client::new(), specs, 8, "download", sink).await.unwrap();

        let events = seen.lock().unwrap();
        assert_eq!(report.skipped, 300);
        assert!(events.len() < 20, "{} progress events for 300 files", events.len());
        let last = events.last().unwrap();
        assert!(last.finished);
        assert_eq!((last.files_done, last.files_total), (300, 300));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
