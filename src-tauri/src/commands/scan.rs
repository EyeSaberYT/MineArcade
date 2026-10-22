//! Instance content scanning: an offline heuristic pass, then an online
//! provenance pass against Modrinth's file database.
//!
//! The two halves answer different questions. The heuristic pass asks "does
//! this file behave like an account stealer?" and needs no network. The
//! provenance pass asks the sharper question the heuristics cannot answer —
//! "is this still byte-for-byte the file the publisher shipped?" — because a
//! mod that has been edited to carry a payload no longer hashes to anything
//! Modrinth has ever published.
//!
//! The provenance pass is strictly best effort. Offline, rate limited, or
//! Modrinth having a bad day, the scan still returns: files come back marked
//! `unchecked` rather than falsely `unknown`, because those mean very
//! different things and a security tool that blurs them is not worth running.

use std::collections::HashMap;
use std::time::Instant;

use tauri::State;

use crate::api::modrinth::ModrinthClient;
use crate::core::instance::game_dir;
use crate::core::scan::{
    apply_provenance, collect_archives, finish, scan_archive, FileReport, Provenance,
    ProvenanceStatus, ScanReport,
};
use crate::error::{LauncherError, Result};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

/// Scan every mod, resource pack, shader pack and datapack in an instance.
///
/// Findings are ordered worst-first by `finish`, so the frontend can render
/// the report as-is.
#[tauri::command]
pub async fn scan_instance(state: State<'_, AppState>, id: String) -> Result<ScanReport> {
    let instance = state.instances.read().await.get(&id)?.clone();
    let dir = game_dir(&state.paths, &instance);
    let started = Instant::now();

    // Pass 1 — offline. Opening and decompressing every archive is blocking
    // work; run it off the async runtime so the UI (and anything else the
    // window is doing) keeps breathing while a big pack is read.
    let root = dir.clone();
    let scanned = tokio::task::spawn_blocking(move || {
        collect_archives(&root)
            .into_iter()
            .map(|path| scan_archive(&path, &root))
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|error| LauncherError::msg(format!("The scan could not run: {error}")))?;

    let mut files: Vec<FileReport> = scanned
        .into_iter()
        // The only way `scan_archive` returns an error is a race — the file
        // vanished between listing and opening. Ordinary read failures are
        // reported as findings so they never disappear silently.
        .filter_map(Result::ok)
        .collect();

    // Pass 2 — online, best effort.
    let hashes: Vec<String> = files.iter().map(|file| file.sha1.clone()).collect();
    let known = lookup_provenance(&state, &hashes).await;
    let provenance_checked = known.is_some();
    if let Some(known) = known {
        apply_provenance(&mut files, &known);
    }

    Ok(finish(files, started, provenance_checked))
}

// ---------------------------------------------------------------------------
// Provenance
// ---------------------------------------------------------------------------

/// Ask Modrinth which of these sha1s it has actually published.
///
/// `None` means the question could not be asked; `Some` means it was answered,
/// including a `Some` of an empty map when the answer is "none of them".
async fn lookup_provenance(
    state: &AppState,
    hashes: &[String],
) -> Option<HashMap<String, Provenance>> {
    if hashes.is_empty() {
        return Some(HashMap::new());
    }

    let client = ModrinthClient::new(state.http.clone(), state.modrinth_base().await);

    // `.ok()?` is deliberate. A failure partway through must not leave the
    // remaining files marked `unknown` when we simply never asked about them,
    // so any failure abandons the lookup entirely and the report says
    // `unchecked`. Batching lives in the client: one request per mod would be
    // hundreds of round trips, and Modrinth rate limits long before they
    // finish.
    let found = client.versions_from_hashes_batched(hashes).await.ok()?;
    let versions: HashMap<String, crate::api::modrinth::Version> = found
        .into_iter()
        .map(|(hash, version)| (hash.to_uppercase(), version))
        .collect();

    // A verified file deserves a readable name, not a bare project id. Titles
    // are cosmetic though, so a failure here costs nothing but the title.
    let titles: HashMap<String, String> = client
        .projects_batched(&project_ids(&versions))
        .await
        .into_iter()
        .map(|project| (project.id, project.title))
        .collect();

    Some(
        versions
            .into_iter()
            .map(|(hash, version)| {
                let label = if version.version_number.is_empty() {
                    version.name
                } else {
                    format!("{} {}", version.name, version.version_number)
                };
                let project = titles.get(&version.project_id).cloned();

                (
                    hash,
                    Provenance {
                        status: ProvenanceStatus::Verified,
                        project,
                        version: Some(label),
                        project_id: Some(version.project_id),
                    },
                )
            })
            .collect(),
    )
}

/// Distinct project ids across a hash lookup, deduplicated so a pack that
/// ships ten files from one project still asks for that title once.
fn project_ids(versions: &HashMap<String, crate::api::modrinth::Version>) -> Vec<String> {
    let mut ids: Vec<String> = versions.values().map(|version| version.project_id.clone()).collect();
    ids.sort();
    ids.dedup();
    ids
}
