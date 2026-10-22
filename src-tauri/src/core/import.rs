//! Importing instances that already belong to another launcher.
//!
//! Two rules shape everything here:
//!
//! * **Nothing another launcher owns is written to.** A scan only reads
//!   metadata, and an import copies the game folder out of it, so the original
//!   keeps working — including for people who still open the other launcher.
//! * **A copy is not a clone.** MineArcade resolves the game itself (client jar,
//!   libraries, assets, natives, loader profile) from its own shared `meta/`
//!   folder, so the parts of a game folder that belong to the *launcher* rather
//!   than to the player are deliberately left behind. [`Skips`] is the single
//!   place that decides which.
//!
//! Reading other launchers means reading formats MineArcade does not control:
//! `mmc-pack.json` (Prism/MultiMC), `minecraftinstance.json` (CurseForge),
//! `launcher_profiles.json` (the Minecraft Launcher) and Modrinth's SQLite
//! database. Every one of them is treated as a hint: whatever cannot be read
//! stays `None` and the user picks it in the import dialog.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::download::{Progress, ProgressSink};
use super::instance::LoaderKind;
use super::paths::read_json;
use crate::error::{LauncherError, Result};

// ---------------------------------------------------------------------------
// What a scan hands back
// ---------------------------------------------------------------------------

/// A launcher MineArcade can read game folders out of.
///
/// The serde spelling of each variant is part of the frontend API: the import
/// dialog sends the same key back when it asks for a candidate to be imported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportSource {
    Prism,
    /// Serialises as `multimc`.
    MultiMC,
    /// Serialises as `curseforge`.
    CurseForge,
    /// Serialises as `modrinth` (the Modrinth App).
    Modrinth,
    /// The official Minecraft Launcher.
    Minecraft,
    /// A folder the user picked by hand.
    Folder,
}

/// One game folder another launcher could hand over.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCandidate {
    /// Unique within a single scan; also the React key.
    pub id: String,
    pub kind: ImportSource,
    /// Display name of the launcher this came from.
    pub source: String,
    /// The name the other launcher shows.
    pub name: String,
    /// Absolute path of the game folder an import would copy.
    pub path: String,
    /// Absolute path of the other launcher's icon for it, when there is one.
    pub icon: Option<String>,
    pub game_version: Option<String>,
    pub loader: Option<LoaderKind>,
    pub loader_version: Option<String>,
    /// Bytes the copy would take, with the skipped folders already excluded.
    pub size: u64,
    /// Jars sitting in the folder's `mods` directory.
    pub mods: usize,
    /// Newest file modification time, in milliseconds since the Unix epoch.
    pub modified: Option<i64>,
    /// What MineArcade could not work out, in the user's words.
    pub note: Option<String>,
}

/// One launcher the scan looked for, whether or not it was there.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSourceInfo {
    pub kind: ImportSource,
    pub label: String,
    /// Where MineArcade looked, even when nothing was found.
    pub path: String,
    /// Whether the launcher looks installed.
    pub found: bool,
    /// Why nothing could be read, when that is worth saying.
    pub note: Option<String>,
}

/// The whole answer to "what could MineArcade import?".
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportScan {
    pub sources: Vec<ImportSourceInfo>,
    pub candidates: Vec<ImportCandidate>,
}

impl ImportCandidate {
    /// Which part of the version manifest this candidate is still waiting on.
    ///
    /// The launchers write `latest-release`/`latest-snapshot` instead of a
    /// version id, and only the command layer can reach the manifest (this
    /// module has no network). `true` means "newest release".
    pub fn wants_latest(&self) -> Option<bool> {
        match self.game_version.as_deref() {
            Some(LATEST_RELEASE) => Some(true),
            Some(LATEST_SNAPSHOT) => Some(false),
            _ => None,
        }
    }
}

/// The placeholders a launcher writes instead of a concrete version id.
const LATEST_RELEASE: &str = "latest-release";
const LATEST_SNAPSHOT: &str = "latest-snapshot";

// ---------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------

impl ImportSource {
    /// The launchers the picker looks for, in the order it lists them.
    const LAUNCHERS: [ImportSource; 5] = [
        ImportSource::Prism,
        ImportSource::MultiMC,
        ImportSource::CurseForge,
        ImportSource::Modrinth,
        ImportSource::Minecraft,
    ];

    /// The name a user recognises.
    pub fn label(self) -> &'static str {
        match self {
            ImportSource::Prism => "Prism Launcher",
            ImportSource::MultiMC => "MultiMC",
            ImportSource::CurseForge => "CurseForge",
            ImportSource::Modrinth => "Modrinth App",
            ImportSource::Minecraft => "Minecraft Launcher",
            ImportSource::Folder => "Chosen folder",
        }
    }

    /// Where this launcher might keep its data, most likely first.
    ///
    /// The same launcher lands in different places per platform (`%APPDATA%`
    /// on Windows, `~/Library/Application Support` on macOS, a data dir on
    /// Linux) and Flatpak adds another copy on Linux, so several paths are
    /// worth checking before concluding it is not installed. CurseForge moved
    /// from `Documents` to the home folder, hence two entries for it too.
    fn roots(self) -> Vec<PathBuf> {
        let home = dirs::home_dir().unwrap_or_default();
        let config = dirs::config_dir().unwrap_or_default();
        let data = dirs::data_dir().unwrap_or_default();

        match self {
            ImportSource::Prism => vec![
                config.join("PrismLauncher"),
                data.join("PrismLauncher"),
                home.join(".var/app/org.prismlauncher.PrismLauncher/data/PrismLauncher"),
            ],
            ImportSource::MultiMC => vec![
                config.join("MultiMC"),
                data.join("MultiMC"),
                // MultiMC's own Linux build lowercases it; older packaging
                // lowercased the whole path.
                data.join("multimc"),
                home.join(".var/app/org.multimc.MultiMC/data/MultiMC"),
            ],
            ImportSource::CurseForge => vec![
                home.join("curseforge/minecraft"),
                home.join("Documents/curseforge/minecraft"),
            ],
            ImportSource::Modrinth => vec![
                config.join("ModrinthApp"),
                data.join("ModrinthApp"),
                home.join(".var/app/com.modrinth.ModrinthApp/data/ModrinthApp"),
            ],
            ImportSource::Minecraft => vec![
                config.join(".minecraft"),
                data.join("minecraft"),
                home.join(".minecraft"),
            ],
            ImportSource::Folder => vec![],
        }
    }

    /// The roots that actually exist, without duplicates.
    ///
    /// On Windows (and macOS) the config and data dirs are the same folder, so
    /// the lists above repeat themselves there.
    fn existing_roots(self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = self
            .roots()
            .into_iter()
            .filter(|root| root.is_dir())
            .collect();
        roots.sort();
        roots.dedup();
        roots
    }

    fn scan_root(self, root: &Path) -> Result<Vec<ImportCandidate>> {
        match self {
            ImportSource::Prism | ImportSource::MultiMC => scan_prism(root),
            ImportSource::CurseForge => scan_curseforge(root),
            ImportSource::Modrinth => scan_modrinth(root),
            ImportSource::Minecraft => scan_vanilla(root),
            ImportSource::Folder => Err(LauncherError::msg(
                "A folder is inspected directly rather than scanned for.",
            )),
        }
    }
}

/// Look for importable instances in every launcher MineArcade knows about.
///
/// This touches a lot of small files, so callers should keep it off the async
/// runtime. Nothing here can fail as a whole: a launcher that is missing or
/// unreadable turns into an entry in `sources` explaining itself.
pub fn scan() -> ImportScan {
    let mut sources = Vec::new();
    let mut candidates = Vec::new();

    for kind in ImportSource::LAUNCHERS {
        let roots = kind.existing_roots();
        let looked_at = kind
            .roots()
            .into_iter()
            .next()
            .map(|path| path.display().to_string())
            .unwrap_or_default();

        let mut note = None;
        let mut found: Vec<ImportCandidate> = Vec::new();
        for root in &roots {
            match kind.scan_root(root) {
                Ok(list) => found.extend(list),
                Err(error) => note = Some(error.to_string()),
            }
        }

        // Two roots of one launcher, or several profiles pointing at the same
        // folder, would otherwise offer the same copy twice.
        dedupe(&mut found);
        // Newest first: the instance someone actually plays is the one they
        // are most likely after.
        found.sort_by_key(|candidate| std::cmp::Reverse(candidate.modified));

        sources.push(ImportSourceInfo {
            kind,
            label: kind.label().to_string(),
            path: looked_at,
            found: !roots.is_empty(),
            note,
        });
        candidates.extend(found);
    }

    ImportScan {
        sources,
        candidates,
    }
}

/// Work out what a hand-picked folder is.
///
/// The user may have pointed at a launcher's data directory, at one instance
/// inside it, or straight at a `.minecraft` folder, so each of those shapes is
/// recognised. When none of them match, the folder is offered as-is: the size,
/// the mod count and the (missing) version are shown, and the user decides.
pub fn inspect(path: &Path) -> Result<Vec<ImportCandidate>> {
    if !path.is_dir() {
        return Err(LauncherError::msg("That folder does not exist."));
    }

    if path.join("launcher_profiles.json").is_file() {
        return scan_vanilla(path);
    }
    if path.join("instances").is_dir() {
        let mut found = scan_prism(path)?;
        found.extend(scan_curseforge(path)?);
        if !found.is_empty() {
            dedupe(&mut found);
            found.sort_by_key(|candidate| std::cmp::Reverse(candidate.modified));
            return Ok(found);
        }
    }

    Ok(vec![folder_candidate(path)?])
}

// ---------------------------------------------------------------------------
// Prism Launcher / MultiMC
// ---------------------------------------------------------------------------

/// Prism and MultiMC keep one folder per instance under `instances/`, each with
/// an `mmc-pack.json` naming the exact components it was built from — the most
/// precise of the formats MineArcade reads.
fn scan_prism(root: &Path) -> Result<Vec<ImportCandidate>> {
    let mut found = Vec::new();
    for entry in read_dir_sorted(&root.join("instances"))? {
        if !entry.is_dir() {
            continue;
        }
        if let Some(candidate) = prism_candidate(&entry)? {
            found.push(candidate);
        }
    }
    Ok(found)
}

/// One Prism/MultiMC instance folder, or `None` when it is not one.
fn prism_candidate(instance: &Path) -> Result<Option<ImportCandidate>> {
    let settings = parse_ini(&instance.join("instance.cfg"));
    let pack = read_json::<MmcPack>(&instance.join("mmc-pack.json"))?;
    if settings.is_empty() && pack.is_none() {
        return Ok(None);
    }

    let (game_version, loader, loader_version) = pack.map(mmc_build).unwrap_or_default();

    let name = settings
        .get("name")
        .filter(|name| !name.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| folder_name(instance));

    // A custom icon is either dropped into the instance folder or kept in the
    // launcher's shared `icons` folder under the key `instance.cfg` names.
    let mut icons = vec![instance.join("icon.png"), instance.join("instance-icon.png")];
    if let Some(key) = settings.get("iconKey").filter(|key| !key.trim().is_empty()) {
        if let Some(data) = instance.parent().and_then(Path::parent) {
            icons.push(data.join("icons").join(key).with_extension("png"));
        }
    }
    let icon = icons.into_iter().find(|path| path.is_file());

    // An instance can keep its game files in a `minecraft` folder (Prism's
    // default) or `.minecraft`; older MultiMC instances use either.
    let game = game_folder(instance);

    finish(Draft {
        kind: ImportSource::Prism,
        name,
        game,
        icon,
        game_version,
        loader,
        loader_version,
        note: None,
    })
    .map(Some)
}

/// The `mmc-pack.json` components MineArcade understands.
fn mmc_build(pack: MmcPack) -> (Option<String>, Option<LoaderKind>, Option<String>) {
    let mut build = (None, None, None);
    for component in pack.components {
        match component.uid.as_str() {
            "net.minecraft" => {
                build.0 = component.version.clone();
            }
            "net.fabricmc.fabric-loader" => {
                build.1 = Some(LoaderKind::Fabric);
                build.2 = component.version.clone();
            }
            "org.quiltmc.quilt-loader" => {
                build.1 = Some(LoaderKind::Quilt);
                build.2 = component.version.clone();
            }
            "net.minecraftforge" => {
                build.1 = Some(LoaderKind::Forge);
                build.2 = component.version.as_deref().and_then(trim_loader_version);
            }
            "net.neoforged" => {
                build.1 = Some(LoaderKind::NeoForge);
                build.2 = component.version.clone();
            }
            _ => {}
        }
    }
    build
}

/// Prism's `mmc-pack.json`.
#[derive(Debug, Deserialize)]
struct MmcPack {
    #[serde(default)]
    components: Vec<MmcComponent>,
}

#[derive(Debug, Deserialize)]
struct MmcComponent {
    uid: String,
    #[serde(default)]
    version: Option<String>,
}

// ---------------------------------------------------------------------------
// CurseForge
// ---------------------------------------------------------------------------

/// The CurseForge app keeps one folder per profile under `Instances/`, and the
/// folder *is* the game folder — there is no `minecraft` level inside it.
fn scan_curseforge(root: &Path) -> Result<Vec<ImportCandidate>> {
    let mut found = Vec::new();
    for entry in read_dir_sorted(&root.join("Instances"))? {
        if !entry.is_dir() {
            continue;
        }
        if let Some(candidate) = curseforge_candidate(&entry)? {
            found.push(candidate);
        }
    }
    Ok(found)
}

/// One CurseForge profile folder, or `None` when it is not one.
fn curseforge_candidate(instance: &Path) -> Result<Option<ImportCandidate>> {
    let Some(profile) = read_json::<CurseForgeInstance>(&instance.join("minecraftinstance.json"))?
    else {
        return Ok(None);
    };

    let (game_version, loader, loader_version) = curseforge_build(&profile);
    let name = profile
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| folder_name(instance));

    finish(Draft {
        kind: ImportSource::CurseForge,
        name,
        game: instance.to_path_buf(),
        icon: first_existing([instance.join("icon.png"), instance.join("icon.jpg")]),
        game_version,
        loader,
        loader_version,
        note: None,
    })
    .map(Some)
}

/// What `minecraftinstance.json` says the profile was built on.
fn curseforge_build(profile: &CurseForgeInstance) -> (Option<String>, Option<LoaderKind>, Option<String>) {
    let Some(loader) = profile.base_mod_loader.as_ref().and_then(|l| l.name.as_deref()) else {
        return (profile.game_version.clone(), None, None);
    };

    // CurseForge names the loader `forge-47.2.0`, `fabric-0.16.9`, and so on.
    let (kind, version) = match loader.split_once('-') {
        Some((kind, version)) => (kind, Some(version.to_string())),
        None => (loader, None),
    };

    (
        profile.game_version.clone(),
        LoaderKind::parse(kind),
        version.filter(|v| !v.trim().is_empty()),
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurseForgeInstance {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    game_version: Option<String>,
    #[serde(default)]
    base_mod_loader: Option<CurseForgeLoader>,
}

#[derive(Debug, Deserialize)]
struct CurseForgeLoader {
    #[serde(default)]
    name: Option<String>,
}

// ---------------------------------------------------------------------------
// The Minecraft Launcher
// ---------------------------------------------------------------------------

/// The official launcher keeps one profile list describing every install it can
/// start, and most of those profiles share the single `.minecraft` folder.
fn scan_vanilla(root: &Path) -> Result<Vec<ImportCandidate>> {
    let Some(profiles) = read_json::<VanillaProfiles>(&root.join("launcher_profiles.json"))? else {
        // A game folder with no profile list is still worth importing; the
        // user picks the version.
        return Ok(vec![finish(Draft {
            kind: ImportSource::Minecraft,
            name: folder_name(root),
            game: root.to_path_buf(),
            icon: None,
            game_version: None,
            loader: None,
            loader_version: None,
            note: Some("The launcher's profile list is missing, so pick a version.".into()),
        })?]);
    };

    // Custom profiles first, then the newest release, then the snapshot, so
    // that when several profiles share one folder the most specific wins.
    let mut ordered: Vec<(u8, String, VanillaProfile)> = profiles
        .profiles
        .into_iter()
        .map(|(id, profile)| {
            let weight = match profile.kind.as_deref() {
                Some("custom") => 0,
                Some("latest-release") => 1,
                Some("latest-snapshot") => 2,
                _ => 3,
            };
            (weight, id, profile)
        })
        .collect();
    ordered.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    let mut found = Vec::new();
    for (_, _, profile) in ordered {
        let game = match profile.game_dir.as_deref().map(str::trim) {
            Some(dir) if !dir.is_empty() => {
                let dir = PathBuf::from(dir);
                if dir.is_absolute() {
                    dir
                } else {
                    root.join(dir)
                }
            }
            _ => root.to_path_buf(),
        };
        if !game.is_dir() {
            continue;
        }

        found.push(vanilla_candidate(&game, &profile)?);
    }

    Ok(found)
}

/// One launcher profile, with its version id turned into MineArcade's own fields.
fn vanilla_candidate(game: &Path, profile: &VanillaProfile) -> Result<ImportCandidate> {
    let version_id = profile.last_version_id.clone().unwrap_or_default();
    let (mut game_version, loader, loader_version) = parse_version_id(&version_id);
    let note = None;

    // `latest-release` and `latest-snapshot` are placeholders: keep them as-is
    // so the command layer can resolve them against the version manifest.
    if version_id == LATEST_RELEASE {
        game_version = Some(LATEST_RELEASE.to_string());
    } else if version_id == LATEST_SNAPSHOT {
        game_version = Some(LATEST_SNAPSHOT.to_string());
    }

    let name = match profile.name.trim() {
        "" => match profile.kind.as_deref() {
            Some("latest-release") => "Latest release".to_string(),
            Some("latest-snapshot") => "Latest snapshot".to_string(),
            _ => folder_name(game),
        },
        other => other.to_string(),
    };

    finish(Draft {
        kind: ImportSource::Minecraft,
        name,
        game: game.to_path_buf(),
        icon: None,
        game_version,
        loader,
        loader_version,
        note,
    })
}

/// `launcher_profiles.json`, of which MineArcade only needs the profile list.
#[derive(Debug, Deserialize)]
struct VanillaProfiles {
    #[serde(default)]
    profiles: BTreeMap<String, VanillaProfile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VanillaProfile {
    #[serde(default)]
    name: String,
    #[serde(default)]
    game_dir: Option<String>,
    #[serde(default)]
    last_version_id: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

/// Pull the loader out of a launcher-installed version json.
///
/// A modded install lists the loader it patched in among its libraries, which
/// is far more reliable than guessing from the version id.
fn loader_from_version_json(json: &serde_json::Value) -> (Option<LoaderKind>, Option<String>) {
    if let Some(libraries) = json.get("libraries").and_then(|l| l.as_array()) {
        for library in libraries {
            let Some(name) = library.get("name").and_then(|n| n.as_str()) else {
                continue;
            };
            // Maven coordinates: `group:artifact:version`.
            let coordinates: Vec<&str> = name.split(':').collect();
            let kind = match coordinates.as_slice() {
                ["net.fabricmc", "fabric-loader", ..] => Some(LoaderKind::Fabric),
                ["org.quiltmc", "quilt-loader", ..] => Some(LoaderKind::Quilt),
                ["net.minecraftforge", "forge", ..] => Some(LoaderKind::Forge),
                ["net.neoforged", "neoforge", ..] => Some(LoaderKind::NeoForge),
                _ => None,
            };
            if let Some(kind) = kind {
                let version = coordinates.get(2).copied().unwrap_or_default();
                return (Some(kind), trim_loader_version(version));
            }
        }
    }

    // Old Forge builds predate that library layout and only say so in the main
    // class they launch.
    let main = json.get("mainClass").and_then(|m| m.as_str()).unwrap_or("");
    let kind = if main.contains("fabric") {
        Some(LoaderKind::Fabric)
    } else if main.contains("quilt") {
        Some(LoaderKind::Quilt)
    } else if main.contains("neoforged") {
        Some(LoaderKind::NeoForge)
    } else if main.contains("minecraftforge") || main.contains("modlauncher") {
        Some(LoaderKind::Forge)
    } else {
        None
    };
    (kind, None)
}

// ---------------------------------------------------------------------------
// Modrinth App
// ---------------------------------------------------------------------------

/// The Modrinth App keeps its profiles in a SQLite database, in one of two
/// shapes: the current `instances` table, where the version and loader live in
/// the content set an instance is built on, and an older single `profiles`
/// table that held them itself.
///
/// Both are read, and a database MineArcade cannot make sense of (a future
/// migration, a corrupt file) turns into "nothing found" rather than an error.
fn scan_modrinth(root: &Path) -> Result<Vec<ImportCandidate>> {
    let database = root.join("app.db");
    if !database.is_file() {
        return Err(LauncherError::msg("Modrinth App has no app.db here."));
    }
    let connection = open_database(&database)?;

    let candidates = if has_table(&connection, "instances")? {
        modrinth_instances(&connection, root)?
    } else if has_table(&connection, "profiles")? {
        modrinth_profiles(&connection, root)?
    } else {
        return Err(LauncherError::msg(
            "Modrinth App's database has no instance table MineArcade recognises.",
        ));
    };

    Ok(candidates)
}

/// The current Modrinth App schema.
fn modrinth_instances(
    connection: &rusqlite::Connection,
    root: &Path,
) -> Result<Vec<ImportCandidate>> {
    let columns = table_columns(connection, "instances")?;

    // The version and the loader were split out into content sets; on an
    // instance that has not been built yet they are simply absent.
    let builds_join = has_table(connection, "instance_content_sets")?
        && has_column(&columns, "applied_content_set_id");
    let version = if builds_join {
        "c.game_version"
    } else {
        "null"
    };
    let loader = if builds_join { "c.loader" } else { "null" };
    let loader_version = if builds_join {
        "c.loader_version"
    } else {
        "null"
    };
    let icon = if has_column(&columns, "icon_path") {
        "i.icon_path"
    } else {
        "null"
    };
    let join = if builds_join {
        "left join instance_content_sets c on c.id = i.applied_content_set_id"
    } else {
        ""
    };
    // Skip anything mid-install, when the schema says how far along it is.
    let filter = if has_column(&columns, "install_stage") {
        "where i.install_stage is null or i.install_stage = 'installed'"
    } else {
        ""
    };

    let sql = format!(
        "select i.name, i.path, {icon}, {version}, {loader}, {loader_version} \
         from instances i {join} {filter}"
    );

    collect_rows(connection, &sql, root)
}

/// The older Modrinth App schema, where each profile carried its own build.
fn modrinth_profiles(
    connection: &rusqlite::Connection,
    root: &Path,
) -> Result<Vec<ImportCandidate>> {
    let columns = table_columns(connection, "profiles")?;
    let icon = if has_column(&columns, "icon_path") {
        "icon_path"
    } else {
        "null"
    };
    let loader_version = if has_column(&columns, "loader_version") {
        "loader_version"
    } else {
        "null"
    };

    let sql = format!(
        "select name, path, {icon}, game_version, loader, {loader_version} from profiles"
    );

    collect_rows(connection, &sql, root)
}

/// Read the rows both Modrinth schemas boil down to: name, path, icon, version,
/// loader, loader version.
fn collect_rows(
    connection: &rusqlite::Connection,
    sql: &str,
    root: &Path,
) -> Result<Vec<ImportCandidate>> {
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| LauncherError::msg(format!("Modrinth App's instances: {error}")))?;

    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(|error| LauncherError::msg(format!("Modrinth App's instances: {error}")))?;

    let mut found = Vec::new();
    for row in rows {
        let (name, path, icon, game_version, loader, loader_version) =
            row.map_err(|error| LauncherError::msg(format!("Modrinth App's instances: {error}")))?;

        let Some(path) = path else { continue };
        // Paths are stored relative to the profiles folder, but a profile can
        // live anywhere and is then absolute.
        let game = {
            let path = PathBuf::from(&path);
            if path.is_absolute() {
                path
            } else {
                root.join("profiles").join(path)
            }
        };
        if !game.is_dir() {
            continue;
        }

        let loader = loader.as_deref().and_then(LoaderKind::parse);
        let icon = icon
            .as_deref()
            .map(|icon| modrinth_icon(root, icon))
            .filter(|icon| icon.is_file());

        found.push(finish(Draft {
            kind: ImportSource::Modrinth,
            name: name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| folder_name(&game)),
            game,
            icon,
            game_version: game_version.filter(|v| !v.trim().is_empty()),
            loader,
            loader_version: loader_version.filter(|v| !v.trim().is_empty()),
            note: None,
        })?);
    }

    Ok(found)
}

/// Find an icon Modrinth stored as either an absolute path or a file name in
/// one of its caches.
fn modrinth_icon(root: &Path, icon: &str) -> PathBuf {
    let icon = PathBuf::from(icon);
    if icon.is_absolute() {
        return icon;
    }
    for candidate in [
        root.join(&icon),
        root.join("caches").join(&icon),
        root.join("caches/icons").join(&icon),
    ] {
        if candidate.is_file() {
            return candidate;
        }
    }
    root.join(icon)
}

/// Open another application's SQLite database without disturbing it.
///
/// Read-only first, so nothing here can ever write to it. Modrinth keeps the
/// database in WAL mode while it runs and a read-only connection is not always
/// able to attach the write-ahead log, so a plain handle is the fallback; only
/// `SELECT`s are ever run against it. The busy timeout keeps a scan that lands
/// during a checkpoint from failing outright.
fn open_database(path: &Path) -> Result<rusqlite::Connection> {
    let connection = match rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(connection) => connection,
        Err(_) => rusqlite::Connection::open(path)?,
    };
    connection.busy_timeout(Duration::from_secs(2))?;
    Ok(connection)
}

fn has_table(connection: &rusqlite::Connection, name: &str) -> Result<bool> {
    let count: i64 = connection.query_row(
        "select count(*) from sqlite_master where type = 'table' and name = ?1",
        [name],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn table_columns(connection: &rusqlite::Connection, table: &str) -> Result<Vec<String>> {
    let mut statement = connection.prepare(&format!("pragma table_info({table})"))?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(names)
}

/// Whether a table has a column, so a schema that moves on only costs MineArcade
/// the column it lost.
fn has_column(columns: &[String], name: &str) -> bool {
    columns.iter().any(|column| column == name)
}

// ---------------------------------------------------------------------------
// Folders picked by hand
// ---------------------------------------------------------------------------

/// Offer an arbitrary folder, reading whatever metadata happens to be in it.
fn folder_candidate(folder: &Path) -> Result<ImportCandidate> {
    let game = game_folder(folder);
    let (game_version, loader, loader_version) = classify(&game)?;

    let note = if game_version.is_none() {
        Some("MineArcade could not tell which Minecraft version this folder uses, so pick one.".into())
    } else if loader.is_none() && has_mods(&game) {
        Some("This folder has mods but no loader MineArcade could identify — check the loader.".into())
    } else {
        None
    };

    finish(Draft {
        kind: ImportSource::Folder,
        name: folder_name(folder),
        game,
        icon: None,
        game_version,
        loader,
        loader_version,
        note,
    })
}

/// Read what MineArcade can out of a game folder it has no prior knowledge of.
fn classify(game: &Path) -> Result<(Option<String>, Option<LoaderKind>, Option<String>)> {
    if let Some(pack) = read_json::<MmcPack>(&game.join("mmc-pack.json"))? {
        return Ok(mmc_build(pack));
    }
    if let Some(profile) = read_json::<CurseForgeInstance>(&game.join("minecraftinstance.json"))? {
        return Ok(curseforge_build(&profile));
    }

    // A vanilla-launcher style install keeps a version json per version; the
    // one modified most recently is the one that was last played.
    if let Some(json) = newest_version_json(game) {
        let (id_version, id_loader, id_loader_version) = json
            .get("id")
            .and_then(|v| v.as_str())
            .map(parse_version_id)
            .unwrap_or_default();
        let (json_loader, json_loader_version) = loader_from_version_json(&json);
        return Ok((
            json.get("inheritsFrom")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .or(id_version),
            json_loader.or(id_loader),
            json_loader_version.or(id_loader_version),
        ));
    }

    // Nothing that names the loader. What is left is the layout on disk: the
    // libraries a modded install added, the jars in `mods/`, or a Fabric cache.
    Ok((
        None,
        loader_from_library_dir(game).or_else(|| loader_from_mod_jars(game)),
        None,
    ))
}

/// The newest version json an install folder keeps.
fn newest_version_json(game: &Path) -> Option<serde_json::Value> {
    let mut versions: Vec<(SystemTime, PathBuf)> = read_dir_sorted(&game.join("versions"))
        .ok()?
        .into_iter()
        .filter(|dir| dir.is_dir())
        .filter_map(|dir| {
            let file = dir.join(format!("{}.json", dir.file_name()?.to_str()?));
            let modified = std::fs::metadata(&file).ok()?.modified().ok()?;
            Some((modified, file))
        })
        .collect();
    versions.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));

    versions
        .into_iter()
        .find_map(|(_, file)| read_json::<serde_json::Value>(&file).ok().flatten())
}

/// The loader a modded install's own libraries give away.
fn loader_from_library_dir(game: &Path) -> Option<LoaderKind> {
    let libraries = game.join("libraries");
    for (path, kind) in [
        ("net/fabricmc/fabric-loader", LoaderKind::Fabric),
        ("org/quiltmc/quilt-loader", LoaderKind::Quilt),
        ("net/neoforged/neoforge", LoaderKind::NeoForge),
        ("net/minecraftforge/forge", LoaderKind::Forge),
    ] {
        if libraries.join(path).is_dir() {
            return Some(kind);
        }
    }

    // Fabric's remapping cache is left behind by nothing else.
    if game.join(".fabric").is_dir() {
        return Some(LoaderKind::Fabric);
    }
    None
}

/// Read a few mod jars: every loader stamps its own metadata file into the mods
/// built for it, so the first one that identifies itself decides.
fn loader_from_mod_jars(game: &Path) -> Option<LoaderKind> {
    /// Reading every jar in a 200-mod folder would make the scan crawl.
    const SAMPLES: usize = 8;

    let mut jars: Vec<PathBuf> = read_dir_sorted(&game.join("mods"))
        .ok()?
        .into_iter()
        .filter(|path| {
            path.extension()
                .map(|extension| extension.eq_ignore_ascii_case("jar"))
                .unwrap_or(false)
        })
        .collect();
    jars.sort();

    for jar in jars.into_iter().take(SAMPLES) {
        let Ok(file) = std::fs::File::open(&jar) else {
            continue;
        };
        let Ok(mut archive) = zip::ZipArchive::new(file) else {
            continue;
        };
        for (entry, kind) in [
            ("fabric.mod.json", LoaderKind::Fabric),
            ("quilt.mod.json", LoaderKind::Quilt),
            ("META-INF/neoforge.mods.toml", LoaderKind::NeoForge),
            ("META-INF/mods.toml", LoaderKind::Forge),
        ] {
            if archive.by_name(entry).is_ok() {
                return Some(kind);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Version ids
// ---------------------------------------------------------------------------

/// Split a launcher's version id into the pieces MineArcade needs.
///
/// Every installer names the version it adds after what it added, for example
/// `fabric-loader-0.16.9-1.21.1` or `1.21.1-forge-52.0.0`. This is only a
/// fallback for installs with no version json on disk, so it stays deliberately
/// literal and prefers to admit defeat (`None`s) over guessing.
fn parse_version_id(id: &str) -> (Option<String>, Option<LoaderKind>, Option<String>) {
    let id = id.trim();
    if id.is_empty() {
        return (None, None, None);
    }

    // `<loader>-<loader version>-<Minecraft version>`; the split is on the
    // *first* dash of the tail because a Minecraft version can contain one
    // itself (`1.20.5-pre1`).
    for (prefix, kind) in [
        ("fabric-loader-", LoaderKind::Fabric),
        ("quilt-loader-", LoaderKind::Quilt),
    ] {
        if let Some(rest) = id.strip_prefix(prefix) {
            if let Some((loader_version, game)) = rest.split_once('-') {
                return (
                    Some(game.to_string()),
                    Some(kind),
                    trim_loader_version(loader_version),
                );
            }
        }
    }

    // `<Minecraft version>-forge-<forge version>`, and the older
    // `<Minecraft version>-forge<Minecraft version>-<forge version>`.
    if let Some(index) = find_marker(id, "-forge") {
        let game = &id[..index];
        let rest = id[index + "-forge".len()..].trim_start_matches('-');
        let rest = rest.strip_prefix(game).unwrap_or(rest).trim_start_matches('-');
        return (
            Some(game.to_string()),
            Some(LoaderKind::Forge),
            trim_loader_version(rest),
        );
    }

    // NeoForge names its builds after itself, `neoforge-21.1.68`, sometimes
    // with the Minecraft version in front.
    if let Some(rest) = id.strip_prefix("neoforge-") {
        return (
            None,
            Some(LoaderKind::NeoForge),
            trim_loader_version(rest),
        );
    }
    if let Some(index) = find_marker(id, "-neoforge-") {
        return (
            Some(id[..index].to_string()),
            Some(LoaderKind::NeoForge),
            trim_loader_version(&id[index + "-neoforge-".len()..]),
        );
    }

    // A plain id is the Minecraft version itself (`1.21.1`, `24w14a`), though
    // mods that patch the game in place tack their own name on the end
    // (`1.21.1-OptiFine_HD_U_I6`) and then only the front is a version.
    if id.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return (Some(leading_version(id).to_string()), None, None);
    }

    (None, None, None)
}

/// The Minecraft version at the front of an id like `1.21.1-OptiFine_HD_U_I6`.
///
/// `pre`/`rc` suffixes are kept: `1.20.5-pre1` really is a version id.
fn leading_version(id: &str) -> &str {
    let Some((head, tail)) = id.split_once('-') else {
        return id;
    };
    let patch = !tail.starts_with("pre") && !tail.starts_with("rc");
    if is_release_number(head) && patch {
        head
    } else {
        id
    }
}

/// Whether a string is a dotted release number such as `1.21` or `1.21.1`.
fn is_release_number(value: &str) -> bool {
    value.split('.').count() >= 2
        && value
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// Maven versions of Forge used to carry the Minecraft version in front
/// (`1.20.1-47.2.0`); MineArcade wants the loader build alone.
fn trim_loader_version(version: &str) -> Option<String> {
    let version = version.trim();
    if let Some((head, tail)) = version.split_once('-') {
        if is_release_number(head) {
            return non_empty(tail);
        }
    }
    non_empty(version)
}

// ---------------------------------------------------------------------------
// Measuring and copying
// ---------------------------------------------------------------------------

/// What a copy of a game folder would cost.
struct Measured {
    bytes: u64,
    mods: usize,
    modified: Option<i64>,
}

/// A candidate under construction, before its folder has been measured.
struct Draft {
    kind: ImportSource,
    name: String,
    game: PathBuf,
    icon: Option<PathBuf>,
    game_version: Option<String>,
    loader: Option<LoaderKind>,
    loader_version: Option<String>,
    note: Option<String>,
}

/// Measure a draft's folder and turn it into a candidate.
fn finish(draft: Draft) -> Result<ImportCandidate> {
    let measured = measure(&draft.game, &skips_for(draft.kind))?;

    Ok(ImportCandidate {
        id: format!("{}:{}", draft.kind.label(), draft.game.display()),
        kind: draft.kind,
        source: draft.kind.label().to_string(),
        name: draft.name,
        path: draft.game.display().to_string(),
        icon: draft.icon.map(|path| path.display().to_string()),
        game_version: draft.game_version,
        loader: draft.loader,
        loader_version: draft.loader_version,
        size: measured.bytes,
        mods: measured.mods,
        modified: measured.modified,
        note: draft.note,
    })
}

/// Size, mod count and newest modification time, counting only the files an
/// import would actually copy.
fn measure(game: &Path, skips: &Skips) -> Result<Measured> {
    let mut bytes = 0u64;
    let mut modified: Option<SystemTime> = None;

    for entry in walk(game, skips) {
        let entry = entry.map_err(|error| LauncherError::msg(error.to_string()))?;
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }

        bytes += metadata.len();
        if let Ok(time) = metadata.modified() {
            modified = Some(match modified {
                Some(current) if current > time => current,
                _ => time,
            });
        }
    }

    Ok(Measured {
        bytes,
        mods: mod_count(game),
        modified: modified.and_then(epoch_millis),
    })
}

/// Copy a game folder into a new instance's own game folder.
///
/// Only the player's files travel; everything MineArcade rebuilds, and everything
/// the source launcher owns, stays where it is. The folder is measured first so
/// the progress bar has a total, and progress is then reported as bytes land
/// (throttled, since a modpack can be tens of thousands of files).
pub fn copy_game_folder(
    from: &Path,
    to: &Path,
    kind: ImportSource,
    sink: ProgressSink,
) -> Result<()> {
    /// Long enough that the UI is not rewritten per file, short enough that the
    /// bar still moves on a slow disk.
    const REPORT_EVERY: Duration = Duration::from_millis(150);

    let skips = skips_for(kind);
    std::fs::create_dir_all(to)?;

    let mut progress = Progress::new("import", "Measuring the game folder…");
    sink(progress.clone());
    progress.total = measure(from, &skips)?.bytes;
    progress.label = "Copying the game folder…".to_string();

    let mut last_report = Instant::now();
    for entry in walk(from, &skips) {
        let entry = entry.map_err(|error| LauncherError::msg(error.to_string()))?;
        let relative = entry
            .path()
            .strip_prefix(from)
            .map_err(|_| LauncherError::msg("invalid path"))?;
        let target = to.join(relative);

        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if !entry.file_type().is_file() {
            // Symlinks and friends: following them could point the game at
            // files outside the instance.
            continue;
        }

        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(entry.path(), &target)?;

        progress.current += entry.metadata().map(|data| data.len()).unwrap_or(0);
        progress.files_done += 1;
        if last_report.elapsed() >= REPORT_EVERY {
            last_report = Instant::now();
            sink(progress.clone());
        }
    }

    progress.finished = true;
    sink(progress);
    Ok(())
}

/// How many jars (or disabled jars) sit in the folder's `mods` directory.
fn mod_count(game: &Path) -> usize {
    std::fs::read_dir(game.join("mods"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().is_file())
                .count()
        })
        .unwrap_or(0)
}

fn has_mods(game: &Path) -> bool {
    mod_count(game) > 0
}

/// The top-level names an import leaves behind, by source.
///
/// Names are only compared against the top level of the game folder, so a mod
/// that happens to be called `versions` inside `config/` is untouched. The
/// launcher-specific entries are the bookkeeping files of the app being read
/// (which for the Minecraft Launcher includes its cached credentials).
#[derive(Debug, Clone, Copy)]
struct Skips {
    dirs: &'static [&'static str],
    files: &'static [&'static str],
    prefixes: &'static [&'static str],
}

/// Everything MineArcade either rebuilds itself or lets the game recreate.
const ALWAYS_SKIP: &[&str] = &[
    // Resolved from MineArcade's own shared `meta/` folder.
    "assets",
    "libraries",
    "versions",
    "natives",
    "runtime",
    "jre",
    // Rewritten on every launch.
    "logs",
    "crash-reports",
    "debug",
    "webcache",
    "webcache2",
    ".cache",
    // Loader remapping caches.
    ".fabric",
    ".quilt",
];

fn skips_for(kind: ImportSource) -> Skips {
    let mut skips = Skips {
        dirs: ALWAYS_SKIP,
        files: &[],
        prefixes: &[],
    };
    match kind {
        ImportSource::Minecraft => {
            skips.files = MINECRAFT_FILES;
            skips.prefixes = MINECRAFT_PREFIXES;
        }
        ImportSource::CurseForge => skips.files = CURSEFORGE_FILES,
        _ => {}
    }
    skips
}

/// The launcher's own files: profile lists, logs and cached credentials.
const MINECRAFT_FILES: &[&str] = &[
    "clientId_v2.txt",
    "treatment_tags.json",
    "realms_persistence.json",
    "usercache.json",
];
const MINECRAFT_PREFIXES: &[&str] = &["launcher_"];

/// CurseForge's per-profile bookkeeping.
const CURSEFORGE_FILES: &[&str] = &["minecraftinstance.json", ".curseclient"];

impl Skips {
    /// Whether a top-level entry is one an import leaves behind.
    fn covers(&self, entry: &walkdir::DirEntry) -> bool {
        if entry.depth() != 1 {
            return false;
        }
        let name: &str = &entry.file_name().to_string_lossy();
        self.dirs.contains(&name)
            || self.files.contains(&name)
            || self.prefixes.iter().any(|prefix| name.starts_with(*prefix))
    }
}

/// Walk a game folder, pruning everything an import leaves behind.
fn walk<'a>(
    game: &'a Path,
    skips: &'a Skips,
) -> impl Iterator<Item = walkdir::Result<walkdir::DirEntry>> + 'a {
    walkdir::WalkDir::new(game)
        .into_iter()
        .filter_entry(move |entry| !skips.covers(entry))
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// The folder the game itself runs in.
///
/// Prism puts the game in a `minecraft` folder inside the instance, the
/// Minecraft Launcher uses `.minecraft`, and CurseForge profiles *are* the game
/// folder, so the first of those that exists wins.
fn game_folder(folder: &Path) -> PathBuf {
    for name in ["minecraft", ".minecraft"] {
        let candidate = folder.join(name);
        if candidate.is_dir() {
            return candidate;
        }
    }
    folder.to_path_buf()
}

/// A readable name for a folder, ignoring the workaround dots.
fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().trim_start_matches('.').to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Instance".to_string())
}

/// The first of several icon candidates that exists.
fn first_existing<const N: usize>(paths: [PathBuf; N]) -> Option<PathBuf> {
    paths.into_iter().find(|path| path.is_file())
}

/// Directory entries in a stable order, with a missing directory reading as
/// "nothing there" rather than an error.
fn read_dir_sorted(path: &Path) -> Result<Vec<PathBuf>> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Ok(vec![]);
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    Ok(paths)
}

/// A QSettings-style file: one `key=value` per line, which is all of
/// `instance.cfg` MineArcade needs.
fn parse_ini(path: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .collect()
}

/// Case-insensitive search, for markers the installers capitalise
/// inconsistently. ASCII lowercasing preserves byte offsets, so the index is
/// safe to use on the original string.
fn find_marker(haystack: &str, needle: &str) -> Option<usize> {
    haystack.to_ascii_lowercase().find(needle)
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn epoch_millis(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|since| since.as_millis() as i64)
}

/// Drop candidates that point at the same game folder.
fn dedupe(candidates: &mut Vec<ImportCandidate>) {
    let mut seen: Vec<String> = Vec::new();
    candidates.retain(|candidate| {
        let key = if cfg!(windows) {
            candidate.path.to_lowercase()
        } else {
            candidate.path.clone()
        };
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clean scratch directory under the temp folder, keyed by test name so
    /// the tests can run in parallel.
    pub(super) fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("minearcade-import-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    pub(super) fn write(path: impl AsRef<Path>, contents: &str) {
        let path = path.as_ref();
        std::fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
        std::fs::write(path, contents).expect("write file");
    }

    #[test]
    fn reads_a_prism_instance() {
        let root = scratch("prism");
        let instance = root.join("instances/Cool Pack");
        write(
            instance.join("instance.cfg"),
            "InstanceType=OneSix\nname=Cool Pack\niconKey=default\n",
        );
        write(
            instance.join("mmc-pack.json"),
            r#"{"formatVersion": 1, "components": [
                 {"uid": "net.minecraft", "version": "1.21.1"},
                 {"uid": "net.fabricmc.fabric-loader", "version": "0.16.9"}
               ]}"#,
        );
        write(instance.join("minecraft/mods/one.jar"), "jar");
        write(instance.join("minecraft/mods/two.jar"), "jar");
        // Launcher-managed folders must not count towards the copy.
        write(instance.join("minecraft/libraries/lib.jar"), "not copied");
        write(instance.join("minecraft/saves/world/level.dat"), "world");

        let found = scan_prism(&root).expect("scan");

        assert_eq!(found.len(), 1);
        let candidate = &found[0];
        assert_eq!(candidate.name, "Cool Pack");
        assert_eq!(candidate.game_version.as_deref(), Some("1.21.1"));
        assert_eq!(candidate.loader, Some(LoaderKind::Fabric));
        assert_eq!(candidate.loader_version.as_deref(), Some("0.16.9"));
        assert_eq!(candidate.mods, 2);
        assert!(
            candidate.size < 100,
            "the excluded libraries folder must not be measured"
        );
        assert!(candidate.modified.is_some());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reads_a_multimc_forge_instance() {
        let root = scratch("multimc");
        let instance = root.join("instances/Forge Pack");
        write(instance.join("instance.cfg"), "name=Forge Pack\n");
        write(
            instance.join("mmc-pack.json"),
            r#"{"components": [
                 {"uid": "net.minecraft", "version": "1.20.1"},
                 {"uid": "net.minecraftforge", "version": "1.20.1-47.2.0"}
               ]}"#,
        );
        write(instance.join(".minecraft/options.txt"), "fov:0.0\n");

        let found = scan_prism(&root).expect("scan");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].game_version.as_deref(), Some("1.20.1"));
        assert_eq!(found[0].loader, Some(LoaderKind::Forge));
        // The Minecraft version in front of the build is not part of it.
        assert_eq!(found[0].loader_version.as_deref(), Some("47.2.0"));
        assert!(found[0].path.ends_with(".minecraft"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn ignores_a_folder_in_instances_that_is_not_an_instance() {
        let root = scratch("prism-junk");
        std::fs::create_dir_all(root.join("instances/.staging")).expect("dirs");

        assert!(scan_prism(&root).expect("scan").is_empty());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reads_a_curseforge_profile() {
        let root = scratch("curseforge");
        let instance = root.join("Instances/Kitchen Sink");
        write(
            instance.join("minecraftinstance.json"),
            r#"{"name": "Kitchen Sink", "gameVersion": "1.19.2",
                "baseModLoader": {"name": "forge-43.2.0", "type": 1}}"#,
        );
        write(instance.join("mods/mekanism.jar"), "jar");
        write(instance.join("saves/level.dat"), "world");

        let found = scan_curseforge(&root).expect("scan");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Kitchen Sink");
        assert_eq!(found[0].game_version.as_deref(), Some("1.19.2"));
        assert_eq!(found[0].loader, Some(LoaderKind::Forge));
        assert_eq!(found[0].loader_version.as_deref(), Some("43.2.0"));
        assert_eq!(found[0].mods, 1);
        // The profile is its own game folder.
        assert_eq!(Path::new(&found[0].path), instance.as_path());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reads_vanilla_profiles_and_collapses_shared_folders() {
        let root = scratch("vanilla");
        write(
            root.join("launcher_profiles.json"),
            r#"{"profiles": {
                 "a": {"type": "latest-release", "lastVersionId": "latest-release", "name": ""},
                 "b": {"type": "latest-snapshot", "lastVersionId": "latest-snapshot", "name": ""},
                 "c": {"type": "custom", "lastVersionId": "1.21.1-forge-52.0.0",
                       "name": "My Forge", "gameDir": "CUSTOM"}
               }}"#,
        );
        write(root.join("options.txt"), "fov:0.0\n");
        let custom = root.join("elsewhere");
        write(custom.join("options.txt"), "fov:1.0\n");
        // Point the custom profile at the folder that exists. JSON needs the
        // separators written as forward slashes, which the desktop launcher
        // writes itself.
        let text = std::fs::read_to_string(root.join("launcher_profiles.json"))
            .expect("profiles")
            .replace("CUSTOM", &custom.display().to_string().replace('\\', "/"));
        write(root.join("launcher_profiles.json"), &text);

        let mut found = scan_vanilla(&root).expect("scan");
        dedupe(&mut found);
        found.sort_by(|a, b| a.name.cmp(&b.name));

        assert_eq!(found.len(), 2, "the two default profiles share one folder");
        let forge = found.iter().find(|c| c.name == "My Forge").expect("forge");
        assert_eq!(forge.game_version.as_deref(), Some("1.21.1"));
        assert_eq!(forge.loader, Some(LoaderKind::Forge));
        assert_eq!(forge.loader_version.as_deref(), Some("52.0.0"));
        assert_eq!(Path::new(&forge.path), custom.as_path());

        let latest = found.iter().find(|c| c.name == "Latest release").expect("latest");
        // The newest release is resolved against the manifest later on.
        assert_eq!(latest.wants_latest(), Some(true));
        assert_eq!(Path::new(&latest.path), root.as_path());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reads_a_vanilla_install_with_no_profile_list() {
        let root = scratch("vanilla-bare");
        write(root.join("saves/world/level.dat"), "world");

        let found = scan_vanilla(&root).expect("scan");

        assert_eq!(found.len(), 1);
        assert!(found[0].game_version.is_none());
        assert!(found[0].note.is_some(), "an unknown version must be explained");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_launcher_files_are_not_copied() {
        let root = scratch("skip");
        let game = root.join("game");
        write(game.join("options.txt"), "fov:0.0\n");
        write(game.join("saves/world/level.dat"), "world");
        write(game.join("launcher_accounts_microsoft_store.json"), "{\"token\": \"secret\"}");
        write(game.join("launcher_profiles.json"), "{}");
        write(game.join("clientId_v2.txt"), "id");
        write(game.join("assets/indexes/1.21.json"), "{}");

        let target = root.join("copy");
        copy_game_folder(&game, &target, ImportSource::Minecraft, silent()).expect("copy");

        assert!(target.join("options.txt").is_file());
        assert!(target.join("saves/world/level.dat").is_file(), "worlds travel");
        assert!(
            !target.join("launcher_accounts_microsoft_store.json").exists(),
            "cached credentials must never be copied"
        );
        assert!(!target.join("clientId_v2.txt").exists());
        assert!(!target.join("assets").exists(), "assets are MineArcade's own");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_folder_is_recognised_by_its_own_libraries() {
        let root = scratch("folder");
        let game = root.join("game");
        write(game.join("mods/a.jar"), "jar");
        std::fs::create_dir_all(game.join("libraries/net/neoforged/neoforge/21.1.68"))
            .expect("libraries");

        let candidate = folder_candidate(&game).expect("candidate");

        assert_eq!(candidate.loader, Some(LoaderKind::NeoForge));
        assert!(candidate.game_version.is_none());
        assert!(candidate.note.is_some());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn parses_the_version_ids_launchers_write() {
        let cases = [
            ("1.21.1", (Some("1.21.1"), None, None)),
            ("24w14a", (Some("24w14a"), None, None)),
            (
                "fabric-loader-0.16.9-1.21.1",
                (
                    Some("1.21.1"),
                    Some(LoaderKind::Fabric),
                    Some("0.16.9"),
                ),
            ),
            (
                "quilt-loader-0.23.1-1.20.4",
                (Some("1.20.4"), Some(LoaderKind::Quilt), Some("0.23.1")),
            ),
            (
                "fabric-loader-0.15.11-1.20.5-pre1",
                (
                    Some("1.20.5-pre1"),
                    Some(LoaderKind::Fabric),
                    Some("0.15.11"),
                ),
            ),
            (
                "1.21.1-forge-52.0.0",
                (Some("1.21.1"), Some(LoaderKind::Forge), Some("52.0.0")),
            ),
            (
                "1.12.2-forge1.12.2-14.23.5.2859",
                (
                    Some("1.12.2"),
                    Some(LoaderKind::Forge),
                    Some("14.23.5.2859"),
                ),
            ),
            (
                "neoforge-21.1.68",
                (None, Some(LoaderKind::NeoForge), Some("21.1.68")),
            ),
            (
                "1.21.1-neoforge-21.1.68",
                (
                    Some("1.21.1"),
                    Some(LoaderKind::NeoForge),
                    Some("21.1.68"),
                ),
            ),
            // A patched install names itself after the patch, so only the
            // front is a Minecraft version.
            ("1.21.1-OptiFine_HD_U_I6", (Some("1.21.1"), None, None)),
        ];

        for (id, expected) in cases {
            let (version, loader, loader_version) = parse_version_id(id);
            assert_eq!(
                (
                    version.as_deref(),
                    loader,
                    loader_version.as_deref()
                ),
                expected,
                "while parsing {id}"
            );
        }
    }

    #[test]
    fn reads_the_loader_out_of_a_version_json() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"id": "1.20.1-forge-47.2.0", "inheritsFrom": "1.20.1",
                "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
                "libraries": [
                  {"name": "net.minecraftforge:forge:1.20.1-47.2.0"},
                  {"name": "net.minecraft:client:1.20.1"}
                ]}"#,
        )
        .expect("json");

        let (loader, version) = loader_from_version_json(&json);

        assert_eq!(loader, Some(LoaderKind::Forge));
        assert_eq!(version.as_deref(), Some("47.2.0"));
    }

    pub(super) fn silent() -> ProgressSink {
        crate::core::download::silent()
    }
}

#[cfg(test)]
mod modrinth_tests {
    use super::tests::*;
    use super::*;

    /// Build an `app.db` in one of the two shapes Modrinth has shipped.
    fn database(path: &Path, schema: &str, insert: &str) {
        let connection = rusqlite::Connection::open(path).expect("create database");
        connection.execute_batch(schema).expect("schema");
        connection.execute_batch(insert).expect("rows");
    }

    const NEW_SCHEMA: &str = "
        create table instances (
            id text primary key,
            path text not null,
            name text not null,
            icon_path text,
            applied_content_set_id text,
            install_stage text
        );
        create table instance_content_sets (
            id text primary key,
            instance_id text not null,
            name text not null,
            source_kind text not null,
            status text not null,
            game_version text not null,
            loader text not null,
            loader_version text
        );";

    #[test]
    fn reads_the_current_modrinth_schema() {
        let root = scratch("modrinth-new");
        database(
            &root.join("app.db"),
            NEW_SCHEMA,
            "insert into instance_content_sets values
                ('set-1', 'local:1', 'main', 'modpack', 'installed', '1.21.1', 'fabric', '0.16.9');
             insert into instances values
                ('local:1', 'Optium', 'Optium', null, 'set-1', 'installed');
             insert into instances values
                ('local:2', 'Halfway', 'Halfway', null, null, 'installing');",
        );
        write(root.join("profiles/Optium/options.txt"), "fov:0.0\n");
        write(root.join("profiles/Optium/mods/sodium.jar"), "jar");
        write(root.join("profiles/Halfway/options.txt"), "fov:0.0\n");

        let found = scan_modrinth(&root).expect("scan");

        assert_eq!(found.len(), 1, "installs still running are left alone");
        let candidate = &found[0];
        assert_eq!(candidate.name, "Optium");
        assert_eq!(candidate.game_version.as_deref(), Some("1.21.1"));
        assert_eq!(candidate.loader, Some(LoaderKind::Fabric));
        assert_eq!(candidate.loader_version.as_deref(), Some("0.16.9"));
        assert_eq!(candidate.mods, 1);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reads_the_older_modrinth_schema() {
        let root = scratch("modrinth-old");
        database(
            &root.join("app.db"),
            "create table profiles (
                path text not null, name text not null, game_version text not null,
                loader text not null, loader_version text);",
            "insert into profiles values
                ('Old Pack', 'Old Pack', '1.20.1', 'quilt', '0.23.1');",
        );
        write(root.join("profiles/Old Pack/options.txt"), "fov:0.0\n");

        let found = scan_modrinth(&root).expect("scan");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Old Pack");
        assert_eq!(found[0].loader, Some(LoaderKind::Quilt));
        assert_eq!(found[0].game_version.as_deref(), Some("1.20.1"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_database_minearcade_cannot_read_is_not_an_error() {
        let root = scratch("modrinth-odd");
        write(root.join("app.db"), "not a database at all");

        // Either the file is not SQLite, or its tables are unrecognised; both
        // have to come back as "nothing here", explained in the source list.
        let result = scan_modrinth(&root);
        if let Ok(found) = result {
            assert!(found.is_empty());
        }

        std::fs::remove_dir_all(&root).ok();
    }
}
