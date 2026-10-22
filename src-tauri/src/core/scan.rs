//! Heuristic content scanning for user-supplied files.
//!
//! This is deliberately **not** an antivirus engine, and it should never be
//! described as one. A real AV compares against a vast signature database and
//! still gets things wrong. What this module does instead is much narrower and
//! much more honest: it looks for the handful of concrete things a *Minecraft
//! account stealer* actually has to do, and it reports what it found so the
//! user can make their own call.
//!
//! The attack this targets is real and specific. Following the 2023 CurseForge
//! incident, the pattern was: take a legitimate mod jar, add a class that
//! copies `%APPDATA%\.minecraft\launcher_accounts.json` (or a Discord token,
//! or a browser password database), and re-upload it. Doing that leaves
//! fingerprints, and those fingerprints are what we search for:
//!
//! * the jar carries a payload it shouldn't (an `.exe`, a `.ps1`, a dropper),
//! * bytecode that reaches for files only a stealer wants,
//! * bytecode that reaches for the OS in ways mods never need to.
//!
//! Every indicator below states *why* it matters, because a security tool that
//! shouts without explaining trains people to ignore it. Findings are
//! evidence, not verdicts: `Warning` entries such as `ProcessBuilder` are
//! used by a small number of legitimate mods (voice chat, native bridges), and
//! the UI must present them that way.
//!
//! Provenance — whether a file is byte-for-byte the file Modrinth published —
//! lives elsewhere, because it needs the network. Local scanning is the
//! offline half of the story.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::error::Result;

// ---------------------------------------------------------------------------
// Budgets
// ---------------------------------------------------------------------------

/// Entries examined per archive. A mod jar is typically hundreds of classes;
/// this is generous enough to cover them and small enough to stay predictable.
const MAX_ENTRIES: usize = 4_096;

/// Bytes decompressed per archive. Text and bytecode are scanned, bulk assets
/// are not, so this covers real coverage many times over.
const MAX_BYTES: u64 = 48 * 1024 * 1024;

/// Bytes read per entry before giving up on it. Individual class files are a
/// few KB; anything enormous is an asset we don't scan anyway.
const MAX_ENTRY_BYTES: u64 = 4 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Report types
// ---------------------------------------------------------------------------

/// How much weight a finding should carry. Ordered, so reports can sort by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    /// Worth knowing about; not dangerous on its own.
    Info,
    /// Uses a capability mods rarely need. Read the detail before worrying.
    Warning,
    /// Matches the behaviour of an account stealer.
    Critical,
}

/// One concrete thing the scanner noticed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub severity: Severity,
    /// Stable machine code, for tests and for grouping in the UI.
    pub code: String,
    pub summary: String,
    /// Why this matters, in plain language.
    pub detail: String,
    /// Where inside the archive it was found, if it was a specific entry.
    pub entry: Option<String>,
}

/// Where a file came from, established by hash rather than by content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProvenanceStatus {
    /// This exact file is in Modrinth's database, so it is unmodified.
    Verified,
    /// Modrinth has never seen this hash. Means nothing on its own: CurseForge
    /// mods, locally built mods and shader packs are all legitimately absent.
    Unknown,
    /// We did not ask, because the user was offline or rate limited.
    Unchecked,
}

/// The provenance half of a scan: is this the file the publisher actually
/// shipped?
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provenance {
    pub status: ProvenanceStatus,
    /// Modrinth project title, when verified.
    pub project: Option<String>,
    /// Modrinth version number, when verified.
    pub version: Option<String>,
    pub project_id: Option<String>,
}

impl Provenance {
    pub fn unchecked() -> Self {
        Self {
            status: ProvenanceStatus::Unchecked,
            project: None,
            version: None,
            project_id: None,
        }
    }
}

/// The result for a single mod, pack or shader.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileReport {
    /// Path relative to the instance's game directory, e.g. `mods/sodium.jar`.
    pub file: String,
    /// `mod`, `resourcepack`, `shaderpack` or `datapack`.
    pub kind: String,
    pub size: u64,
    pub sha1: String,
    pub findings: Vec<Finding>,
    pub provenance: Provenance,
    /// True when we hit a budget and stopped looking. Coverage is reported
    /// rather than implied, so a clean result is never overstated.
    pub truncated: bool,
}

impl FileReport {
    pub fn severity(&self) -> Option<Severity> {
        self.findings.iter().map(|f| f.severity).max()
    }
}

/// Everything the scan learned about an instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub files: Vec<FileReport>,
    /// Archives actually opened and examined.
    pub examined: usize,
    pub critical: usize,
    pub warnings: usize,
    /// True when the Modrinth hash lookup ran. False means the provenance
    /// fields are all `unchecked`.
    pub provenance_checked: bool,
    pub duration_ms: u64,
}

impl ScanReport {
    /// The files that matter, worst first.
    pub fn flagged(&self) -> impl Iterator<Item = &FileReport> {
        self.files.iter().filter(|f| !f.findings.is_empty())
    }

    pub fn clean(&self) -> bool {
        self.flagged().all(|f| f.severity() != Some(Severity::Critical))
            && self.flagged().all(|f| f.severity() != Some(Severity::Warning))
    }
}

// ---------------------------------------------------------------------------
// Indicators
// ---------------------------------------------------------------------------

/// What kind of entry an indicator is meaningful in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// Bytecode. Most indicators belong here, because class constant pools
    /// hold the exact strings a stealer needs to spell out.
    Bytecode,
    /// Any text-ish entry. Only unambiguous filenames and URLs qualify: pack
    /// JSON is written in English, so anything word-like would false-positive.
    Any,
}

/// One search rule. All `needles` must appear in the same file, which is how
/// the ambiguous cases stay precise — `leveldb` alone means nothing, but
/// `leveldb` next to `discord` means someone is reading Discord's token store.
struct Indicator {
    code: &'static str,
    severity: Severity,
    needles: &'static [&'static str],
    target: Target,
    summary: &'static str,
    detail: &'static str,
}

/// The rule set. Needles are matched case-insensitively against the lowercased
/// bytes of the entry.
///
/// These are deliberately few. Each one is a thing a stealer genuinely must
/// do, so a hit is worth a human's attention; a longer list would mostly buy
/// false positives and train the user to click through warnings.
const INDICATORS: &[Indicator] = &[
    Indicator {
        code: "msa_tokens",
        severity: Severity::Critical,
        needles: &["launcher_accounts.json"],
        target: Target::Any,
        summary: "Reaches for Microsoft account tokens",
        detail: "launcher_accounts.json holds the refresh tokens for Microsoft accounts \
                 signed in through a Minecraft launcher. It is the single most common \
                 target of Minecraft account stealers, and no legitimate mod has any \
                 reason to open it.",
    },
    Indicator {
        code: "msa_tokens_legacy",
        severity: Severity::Critical,
        needles: &["launcher_details.json"],
        target: Target::Any,
        summary: "Reaches for legacy launcher account data",
        detail: "launcher_details.json is the older account file for the same purpose. \
                 Reading it is the same signal as launcher_accounts.json.",
    },
    Indicator {
        code: "discord_token",
        severity: Severity::Critical,
        needles: &["discord", "leveldb"],
        target: Target::Any,
        summary: "Reaches for Discord's token store",
        detail: "Discord keeps its login token in a LevelDB database under your app \
                 data folder. Mods have no reason to touch it, and a stolen Discord \
                 token is a full account takeover.",
    },
    Indicator {
        code: "exfil_webhook",
        severity: Severity::Critical,
        needles: &["discord.com/api/webhooks"],
        target: Target::Any,
        summary: "Contains a Discord webhook",
        detail: "Webhooks are the cheapest way for a stealer to push stolen data \
                 somewhere it controls without running a server. Some legitimate \
                 projects use them for their own telemetry, so read the context.",
    },
    Indicator {
        code: "browser_passwords",
        severity: Severity::Critical,
        needles: &["login data"],
        target: Target::Bytecode,
        summary: "Reaches for saved browser passwords",
        detail: "'Login Data' is the SQLite database Chromium keeps saved passwords \
                 in. Nothing that ships as a Minecraft mod needs it.",
    },
    Indicator {
        code: "wallet_recovery",
        severity: Severity::Critical,
        needles: &["seed phrase"],
        target: Target::Bytecode,
        summary: "Reaches for a crypto wallet recovery phrase",
        detail: "A recovery phrase is worth everything in the wallet. Searching for \
                 one is unambiguous intent.",
    },
    Indicator {
        code: "keystroke_capture",
        severity: Severity::Warning,
        needles: &["java/awt/robot"],
        target: Target::Bytecode,
        summary: "Can capture keystrokes and screen",
        detail: "java.awt.Robot can read keystrokes and take screenshots. A handful of \
                 legitimately interactive tools use it, so treat it as a reason to \
                 look closer rather than as proof.",
    },
    Indicator {
        code: "process_exec",
        severity: Severity::Warning,
        needles: &["java/lang/processbuilder"],
        target: Target::Bytecode,
        summary: "Can start arbitrary processes",
        detail: "ProcessBuilder can launch any program on your machine. Native bridges \
                 and voice chat mods legitimately use it, but a mod that never needs \
                 to spawn a process should not.",
    },
    Indicator {
        code: "script_eval",
        severity: Severity::Warning,
        needles: &["javax/script/scriptengine"],
        target: Target::Bytecode,
        summary: "Evaluates scripts at runtime",
        detail: "A script engine lets a jar run JavaScript it loads at runtime, which \
                 is a common way to keep the real payload out of a static read of the \
                 class files.",
    },
    Indicator {
        code: "shell_invocation",
        severity: Severity::Warning,
        needles: &["powershell.exe"],
        target: Target::Any,
        summary: "Invokes PowerShell",
        detail: "PowerShell is how a first stage downloads and runs everything after \
                 it. Mods have no reason to start a shell.",
    },
    Indicator {
        code: "server_list",
        severity: Severity::Info,
        needles: &["servers.dat"],
        target: Target::Any,
        summary: "Reads your server list",
        detail: "servers.dat lists the servers you join. That is personal, but on its \
                 own it is not dangerous.",
    },
];

/// Executables never belong inside a mod archive. Bundling one is either a
/// build mistake or an attack; either way the user should be told.
const CRITICAL_EXECUTABLES: &[&str] = &[
    "exe", "bat", "cmd", "ps1", "vbs", "vbe", "jse", "wsf", "wsh", "hta", "msi", "scr", "com",
    "pif", "lnk",
];

/// Native libraries. These are legitimate in a real subset of mods — voice
/// chat and Discord integration ship them routinely — so they are `Warning`
/// with an explanation rather than an alarm.
const NATIVE_LIBRARIES: &[&str] = &["dll", "so", "dylib", "node"];

/// Entries worth reading the contents of. Anything else (textures, sounds,
/// NBT) is bulk with no strings that could trip an indicator.
fn scan_target(extension: &str) -> Option<Target> {
    match extension {
        "class" => Some(Target::Bytecode),
        "json" | "cfg" | "conf" | "properties" | "xml" | "txt" | "mcmeta" | "fml" | "toml"
        | "yml" | "yaml" | "lang" => Some(Target::Any),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------

/// Substring search with a first-byte prefilter.
///
/// `windows().any()` is correct but recompares the needle from scratch at
/// every offset; skipping ahead to each candidate first byte makes the megabyte
/// scans in a big modpack cost about what you'd expect.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    let first = needle[0];
    let mut start = 0;

    loop {
        let Some(offset) = haystack[start..].iter().position(|&byte| byte == first) else {
            return false;
        };
        let index = start + offset;
        let end = index + needle.len();
        if end > haystack.len() {
            return false;
        }
        if &haystack[index..end] == needle {
            return true;
        }
        start = index + 1;
    }
}

/// Test one entry's contents against every indicator that applies to it.
fn apply_indicators(contents: &[u8], entry: &str, is_bytecode: bool) -> Vec<Finding> {
    let lowered: Vec<u8> = contents.iter().map(u8::to_ascii_lowercase).collect();

    INDICATORS
        .iter()
        .filter(|indicator| {
            // Text written by humans (pack JSON, lang files) is English, so the
            // word-shaped indicators only get to speak for bytecode.
            is_bytecode || indicator.target == Target::Any
        })
        .filter(|indicator| {
            indicator
                .needles
                .iter()
                .all(|needle| contains(&lowered, needle.as_bytes()))
        })
        .map(|indicator| Finding {
            severity: indicator.severity,
            code: indicator.code.to_string(),
            summary: indicator.summary.to_string(),
            detail: indicator.detail.to_string(),
            entry: Some(entry.to_string()),
        })
        .collect()
}

/// SHA-1 of a file, hex encoded. Modrinth's file database is keyed by this.
pub fn sha1_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    let mut hasher = Sha1::new();
    hasher.update(&bytes);
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Scan a single archive and describe what is inside it.
///
/// Failure to open the file is an error; failure to *parse* it as a zip is a
/// finding, because a `.jar` that is not a jar is itself the interesting fact.
pub fn scan_archive(path: &Path, relative_to: &Path) -> Result<FileReport> {
    let metadata = fs::metadata(path)?;
    let size = metadata.len();
    let sha1 = sha1_file(path)?;

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    // The parent folder decides whether this is a mod or a pack, using the
    // same map the install side uses, so `mods/foo.jar` reports `mod` rather
    // than the folder's own name (`mods`).
    let folder = path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy());
    let kind = kind_for_folder(folder.as_deref()).to_string();

    let mut findings: Vec<Finding> = Vec::new();
    let mut truncated = false;

    let Ok(file) = fs::File::open(path) else {
        findings.push(Finding {
            severity: Severity::Warning,
            code: "unreadable".to_string(),
            summary: "Could not be opened".to_string(),
            detail: format!(
                "MineArcade could not read {file_name}. It may be locked by another \
                 process, or the download may have been interrupted."
            ),
            entry: None,
        });
        return Ok(FileReport {
            file: display_relative(path, relative_to),
            kind,
            size,
            sha1,
            findings,
            provenance: Provenance::unchecked(),
            truncated: false,
        });
    };

    let mut archive = match zip::ZipArchive::new(file) {
        Ok(archive) => archive,
        Err(error) => {
            findings.push(Finding {
                severity: Severity::Warning,
                code: "not_an_archive".to_string(),
                summary: "Not a readable jar or pack".to_string(),
                detail: format!(
                    "MineArcade could not open {file_name} as an archive ({error}). A file \
                     with a .jar or .zip extension that is not really one is worth \
                     checking: renaming an executable is a common way to get someone \
                     to run it by double-clicking."
                ),
                entry: None,
            });
            return Ok(FileReport {
                file: display_relative(path, relative_to),
                kind,
                size,
                sha1,
                findings,
                provenance: Provenance::unchecked(),
                truncated: false,
            });
        }
    };

    let mut examined_entries = 0usize;
    let mut examined_bytes = 0u64;

    for index in 0..archive.len() {
        if examined_entries >= MAX_ENTRIES || examined_bytes >= MAX_BYTES {
            truncated = true;
            break;
        }

        let Ok(mut entry) = archive.by_index(index) else {
            // A single unreadable entry is not worth failing the whole report.
            continue;
        };

        let entry_name = entry.name().to_string();
        if entry.is_dir() || entry_name.ends_with('/') {
            continue;
        }
        examined_entries += 1;

        let extension = Path::new(&entry_name)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();

        // Bundled executables.
        if CRITICAL_EXECUTABLES.contains(&extension.as_str()) {
            findings.push(Finding {
                severity: Severity::Critical,
                code: "embedded_executable".to_string(),
                summary: format!("Contains a .{extension} file"),
                detail: format!(
                    "Jars and packs never need to carry programs of their own. A \
                     {extension} file inside {file_name} is either a build mistake or \
                     a payload, and it is worth deleting regardless."
                ),
                entry: Some(entry_name.clone()),
            });
        } else if NATIVE_LIBRARIES.contains(&extension.as_str()) {
            findings.push(Finding {
                severity: Severity::Warning,
                code: "native_library".to_string(),
                summary: format!("Contains a native .{extension} library"),
                detail: format!(
                    "{file_name} bundles native code. Voice chat and Discord \
                     integration mods do this routinely, so this is usually fine — \
                     but native code is not inspected by the JVM and runs with full \
                     permissions."
                ),
                entry: Some(entry_name.clone()),
            });
        }

        let Some(target) = scan_target(&extension) else {
            continue;
        };

        // Read only as much as the entry warrants.
        let limit = entry.size().min(MAX_ENTRY_BYTES);
        let mut buffer = Vec::with_capacity(limit as usize);
        if entry
            .by_ref()
            .take(limit)
            .read_to_end(&mut buffer)
            .is_err()
        {
            continue;
        }
        examined_bytes += buffer.len() as u64;
        if buffer.len() as u64 >= MAX_ENTRY_BYTES {
            truncated = true;
        }

        findings.extend(apply_indicators(
            &buffer,
            &entry_name,
            target == Target::Bytecode,
        ));
    }

    Ok(FileReport {
        file: display_relative(path, relative_to),
        kind,
        size,
        sha1,
        findings,
        provenance: Provenance::unchecked(),
        truncated,
    })
}

/// Turn an absolute path into the stable, instance-relative form the UI shows.
fn display_relative(path: &Path, relative_to: &Path) -> String {
    path.strip_prefix(relative_to)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Every user-supplied content directory, as `(folder, Modrinth kind)`.
const CONTENT_FOLDERS: &[(&str, &str)] = &[
    ("mods", "mod"),
    ("resourcepacks", "resourcepack"),
    ("shaderpacks", "shaderpack"),
    ("datapacks", "datapack"),
];

/// The content kind for a file, decided by the folder it lives in.
fn kind_for_folder(folder: Option<&str>) -> &'static str {
    CONTENT_FOLDERS
        .iter()
        .find(|(name, _)| Some(*name) == folder)
        .map(|(_, kind)| *kind)
        .unwrap_or("mod")
}

/// Content folders worth scanning, as `(folder name, content kind)`.
pub fn content_folders(game_dir: &Path) -> Vec<(&'static str, &'static str)> {
    CONTENT_FOLDERS
        .iter()
        .filter(|(folder, _)| game_dir.join(folder).is_dir())
        .copied()
        .collect()
}

/// Every scannable archive under an instance's game directory.
pub fn collect_archives(game_dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();

    for (folder, _) in content_folders(game_dir) {
        // Pack folders can contain sub-folders, so walk rather than list.
        for entry in walkdir::WalkDir::new(game_dir.join(folder)).follow_links(false) {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_file() {
                continue;
            }
            if is_scannable(entry.path()) {
                found.push(entry.path().to_path_buf());
            }
        }
    }

    found.sort();
    found
}

/// Packs ship as `.zip` alongside `.jar`; the Bedrock-ish formats are included
/// so an imported pack is still covered.
fn is_scannable(path: &Path) -> bool {
    matches!(
        path.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref(),
        Some("jar" | "zip" | "mcpack" | "mcaddon")
    )
}

// ---------------------------------------------------------------------------
// Provenance
// ---------------------------------------------------------------------------

/// Fold Modrinth's hash lookup results back onto the reports.
///
/// Keys are sha1, upper-cased on both sides (Modrinth echoes them however it
/// likes, so normalising here rather than trusting the response keeps this
/// robust). A hash the lookup knew becomes `verified`; every other file
/// becomes `unknown`, which is a statement about Modrinth's database and
/// nothing more — CurseForge mods, local builds and shader packs are all
/// legitimately absent from it.
pub fn apply_provenance(files: &mut [FileReport], known: &HashMap<String, Provenance>) {
    for file in files.iter_mut() {
        let key = file.sha1.to_uppercase();
        file.provenance = known.get(&key).cloned().unwrap_or(Provenance {
            status: ProvenanceStatus::Unknown,
            project: None,
            version: None,
            project_id: None,
        });
    }
}

/// Build the final report from per-file results, with the counts the UI banner
/// needs and consistent worst-first ordering.
pub fn finish(mut files: Vec<FileReport>, started: Instant, provenance_checked: bool) -> ScanReport {
    files.sort_by(|a, b| {
        b.severity()
            .cmp(&a.severity())
            .then_with(|| a.file.cmp(&b.file))
    });

    let critical = files
        .iter()
        .filter(|f| f.severity() == Some(Severity::Critical))
        .count();
    let warnings = files
        .iter()
        .filter(|f| f.severity() == Some(Severity::Warning))
        .count();

    ScanReport {
        examined: files.len(),
        files,
        critical,
        warnings,
        provenance_checked,
        duration_ms: started.elapsed().as_millis() as u64,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A distinct root per fixture. Cargo runs tests in parallel and these
    /// tests all share the system temp dir, so a shared root means one test
    /// deletes another's archive mid-scan.
    fn root_for(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("minearcade-scan-test-{}-{name}", std::process::id()))
    }

    /// Write a real archive into a `mods/` folder under the system temp dir, so
    /// `scan_archive` gets exercised the same way the command calls it —
    /// including the path-relative reporting and the kind detection.
    fn archive(name: &str, entries: &[(&str, &[u8])]) -> PathBuf {
        let root = root_for(name);
        let mods = root.join("mods");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&mods).unwrap();

        let path = mods.join(name);
        let file = fs::File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options =
            zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        for (entry_name, bytes) in entries {
            writer.start_file(*entry_name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
        path
    }

    /// Scan a fixture the way the command does: relative paths are rooted at
    /// the game directory, one level above `mods/`.
    fn scan(path: &Path) -> FileReport {
        let game_dir = path.parent().and_then(|p| p.parent()).unwrap();
        scan_archive(path, game_dir).unwrap()
    }

    /// Raw, non-archive fixture for the "this .jar is not a jar" case.
    fn raw(name: &str, bytes: &[u8]) -> PathBuf {
        let root = root_for(name);
        let mods = root.join("mods");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&mods).unwrap();
        let path = mods.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn codes(report: &FileReport) -> Vec<&str> {
        report.findings.iter().map(|f| f.code.as_str()).collect()
    }

    fn severity_of(report: &FileReport, code: &str) -> Severity {
        report
            .findings
            .iter()
            .find(|f| f.code == code)
            .unwrap_or_else(|| panic!("expected finding {code}, got {:?}", report.findings))
            .severity
    }

    /// A representative well-behaved mod: metadata, a class and an asset.
    #[test]
    fn a_clean_mod_reports_no_findings() {
        let path = archive(
            "clean.jar",
            &[
                (
                    "fabric.mod.json",
                    br#"{"id":"example","name":"Example","version":"1.0"}"#,
                ),
                (
                    "net/example/Foo.class",
                    b"\xca\xfe\xba\xbehello net/example/Foo java/lang/Object String",
                ),
                ("assets/example/text.txt", b"a texture, nothing to see"),
            ],
        );

        let report = scan(&path);
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert_eq!(report.file, "mods/clean.jar");
        assert_eq!(report.kind, "mod");
        assert!(!report.truncated);
        assert_eq!(report.provenance.status, ProvenanceStatus::Unchecked);
    }

    /// The headline case: a jar that reaches for Microsoft account tokens.
    #[test]
    fn flags_a_steerer_after_launcher_accounts() {
        let path = archive(
            "stealer.jar",
            &[(
                "evil/St.class",
                b"\xca\xfe\xba\xbe launcher_accounts.json %APPDATA%",
            )],
        );

        let report = scan(&path);
        assert_eq!(codes(&report), ["msa_tokens"]);
        assert_eq!(severity_of(&report, "msa_tokens"), Severity::Critical);
        assert_eq!(
            report.findings[0].entry.as_deref(),
            Some("evil/St.class"),
            "the finding should say which entry it came from"
        );
        // The explanation must stand on its own for a non-technical user.
        assert!(report.findings[0].detail.contains("launcher_accounts.json"));
    }

    /// `leveldb` on its own means nothing; beside `discord` it means a stolen
    /// token. This is why indicators require every needle, not any of them.
    #[test]
    fn co_occurrence_is_required_before_discord_token_fires() {
        let benign = archive(
            "leveldb-only.jar",
            &[("a.class", b"\xca\xfe\xba\xbe leveldb for world storage")],
        );
        let report = scan(&benign);
        assert!(codes(&report).is_empty(), "{:?}", report.findings);

        let hostile = archive(
            "discord.jar",
            &[(
                "a.class",
                b"\xca\xfe\xba\xbe C:\\discord\\Local Storage\\leveldb",
            )],
        );
        let report = scan(&hostile);
        assert_eq!(codes(&report), ["discord_token"]);
    }

    /// Word-shaped indicators stay out of text files, because pack JSON and
    /// lang files are written in English and would cry wolf. Concrete paths
    /// and URLs apply everywhere: no amount of prose makes
    /// `launcher_accounts.json` a coincidence.
    #[test]
    fn english_indicators_are_not_applied_to_text_entries() {
        let path = archive(
            "noisy.zip",
            &[(
                "assets/pack/pack.json",
                br#"{"description":"seed phrase login data"}"#,
            )],
        );

        let report = scan(&path);
        assert!(report.findings.is_empty(), "{:?}", report.findings);

        // A concrete path still fires from a text file...
        let path = archive(
            "paths.zip",
            &[(
                "assets/pack/pack.json",
                br#"{"note":"reads launcher_accounts.json"}"#,
            )],
        );
        let report = scan(&path);
        assert_eq!(codes(&report), ["msa_tokens"]);

        // ...and the word-shaped ones do fire from bytecode.
        let path = archive(
            "noisy.jar",
            &[("a.class", b"\xca\xfe\xba\xbe seed phrase login data")],
        );
        let report = scan(&path);
        assert!(codes(&report).contains(&"wallet_recovery"));
        assert!(codes(&report).contains(&"browser_passwords"));
    }

    /// Bundled programs are never legitimate inside a mod.
    #[test]
    fn flags_embedded_programs_and_natives_separately() {
        let path = archive(
            "mixed.jar",
            &[
                ("dropper.exe", b"MZ payload"),
                ("launch.bat", b"start payload"),
                ("libvoice.dll", b"MZ native"),
                ("a.class", b"\xca\xfe\xba\xbe hello"),
            ],
        );

        let report = scan(&path);
        assert_eq!(severity_of(&report, "embedded_executable"), Severity::Critical);
        // Natives are real in voice chat mods, so they must not scream.
        assert_eq!(severity_of(&report, "native_library"), Severity::Warning);
        assert_eq!(
            report
                .findings
                .iter()
                .find(|f| f.code == "embedded_executable")
                .unwrap()
                .entry
                .as_deref(),
            Some("dropper.exe")
        );
    }

    /// A `.jar` that is not an archive is the interesting fact, not a failure.
    #[test]
    fn a_file_that_is_not_a_jar_is_a_finding_not_an_error() {
        let path = raw(
            "renamed.jar",
            b"MZ\x90\x00 this is an executable pretending to be a mod",
        );

        let report = scan(&path);
        assert_eq!(codes(&report), ["not_an_archive"]);
        assert_eq!(severity_of(&report, "not_an_archive"), Severity::Warning);
        assert_eq!(report.file, "mods/renamed.jar");
    }

    /// Finding order in a report is what the UI shows, so worst must come first.
    #[test]
    fn reports_are_sorted_worst_first_and_clean_files_sink() {
        let started = Instant::now();
        let file = |name: &str, severity: Option<Severity>| FileReport {
            file: name.to_string(),
            kind: "mod".to_string(),
            size: 0,
            sha1: String::new(),
            findings: severity
                .into_iter()
                .map(|s| Finding {
                    severity: s,
                    code: "x".to_string(),
                    summary: "s".to_string(),
                    detail: "d".to_string(),
                    entry: None,
                })
                .collect(),
            provenance: Provenance::unchecked(),
            truncated: false,
        };

        let report = finish(
            vec![
                file("a-clean.jar", None),
                file("c-critical.jar", Some(Severity::Critical)),
                file("b-info.jar", Some(Severity::Info)),
                file("d-warning.jar", Some(Severity::Warning)),
            ],
            started,
            false,
        );

        let order: Vec<&str> = report.files.iter().map(|f| f.file.as_str()).collect();
        assert_eq!(
            order,
            [
                "c-critical.jar",
                "d-warning.jar",
                "b-info.jar",
                "a-clean.jar"
            ]
        );
        assert_eq!(report.critical, 1);
        assert_eq!(report.warnings, 1);
        assert_eq!(report.examined, 4);
        assert!(!report.clean());
    }

    /// Provenance is a map of sha1 -> project, and hashes Modrinth did not
    /// recognise must come back as unknown rather than silently verified.
    #[test]
    fn provenance_only_verifies_hashes_modrinth_knew() {
        let mut files = vec![
            FileReport {
                file: "mods/good.jar".into(),
                kind: "mod".into(),
                size: 1,
                // We hash to lower case; Modrinth echoes upper case. The
                // lookup must not care which side capitalised it.
                sha1: "aaaa".into(),
                findings: vec![],
                provenance: Provenance::unchecked(),
                truncated: false,
            },
            FileReport {
                file: "mods/odd.jar".into(),
                kind: "mod".into(),
                size: 1,
                sha1: "BBBB".into(),
                findings: vec![],
                provenance: Provenance::unchecked(),
                truncated: false,
            },
        ];

        let mut known = HashMap::new();
        known.insert(
            "AAAA".to_string(),
            Provenance {
                status: ProvenanceStatus::Verified,
                project: Some("Sodium".into()),
                version: Some("Sodium 0.6.0".into()),
                project_id: Some("AANobbMI".into()),
            },
        );
        apply_provenance(&mut files, &known);

        assert_eq!(files[0].provenance.status, ProvenanceStatus::Verified);
        assert_eq!(files[0].provenance.project.as_deref(), Some("Sodium"));
        assert_eq!(files[0].provenance.version.as_deref(), Some("Sodium 0.6.0"));
        assert_eq!(files[1].provenance.status, ProvenanceStatus::Unknown);
        assert_eq!(files[1].provenance.project, None);
    }

    /// The substring helper is the whole engine; make sure the awkward cases
    /// (needle at the very end, repeated first byte, haystack too short) work.
    #[test]
    fn contains_handles_the_edges() {
        assert!(contains(b"launcher_accounts.json", b"launcher_accounts.json"));
        assert!(contains(b"abcabcabc", b"abc"));
        assert!(contains(b"xx launcher_accounts.json", b"accounts.json"));
        assert!(contains(b"aaa", b"a"));
        assert!(!contains(b"launcher_accounts.json", b"launcher_accounts.json.bak"));
        assert!(!contains(b"abc", b"abcdef"));
        assert!(!contains(b"", b"a"));
        // An empty needle matches nothing: a typo'd rule must not light up
        // every file in the instance.
        assert!(!contains(b"abc", b""));
    }
}
