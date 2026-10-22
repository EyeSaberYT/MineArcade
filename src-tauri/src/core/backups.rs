//! World backups: zipping a world folder, and putting one back.
//!
//! A world lives in `<game dir>/saves/<world>`, and MineArcade writes its backups to
//! `<instance>/backups/<world>-<stamp>.zip` — beside the game directory rather
//! than inside it, so the game never sees them and exporting an instance as a
//! modpack does not drag them along.
//!
//! Restoring is the only operation in MineArcade that can destroy work, so it never
//! deletes first: the world standing in the way is zipped into the backup folder
//! before anything is removed, which makes a restore undoable even if it fails
//! halfway through.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{LauncherError, Result};

/// Folder the game keeps its worlds in, relative to a game directory.
pub const SAVES_DIR: &str = "saves";

/// Folder MineArcade keeps backups in, relative to an instance directory.
pub const BACKUPS_DIR: &str = "backups";

/// The file that makes a folder a world. The game lists a world when it has one,
/// so MineArcade uses the same rule rather than inventing a second definition.
const LEVEL_FILE: &str = "level.dat";

/// Icon the game writes beside a world once it has been played.
const ICON_FILE: &str = "icon.png";

/// One world folder in the game's `saves/`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct World {
    /// Folder name, which is what the game shows in its world list.
    pub name: String,
    /// Absolute path, for revealing and previewing.
    pub path: String,
    /// Bytes on disk, everything the world owns.
    pub size: u64,
    /// When the world was last played. RFC 3339, like every other timestamp the
    /// frontend renders.
    pub modified: Option<String>,
    /// Absolute path to the game's `icon.png`, when it has drawn one. The file
    /// lives inside the world, so the UI can preview it without a second call.
    pub icon: Option<String>,
}

/// One `.zip` in the instance's backup folder.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Backup {
    /// File name. Restoring and deleting refer to a backup by this name, never
    /// by a path, so the UI can never point MineArcade outside the backup folder.
    pub name: String,
    /// Absolute path, for revealing.
    pub path: String,
    /// The world it was taken from, read back out of the file name.
    pub world: String,
    pub size: u64,
    /// When the file was written, i.e. when the backup was taken.
    pub created: Option<String>,
}

/// What a restore did, so the UI can say where the replaced world went.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Restored {
    pub world: String,
    /// The safety backup of the world that was replaced, when there was one.
    pub safety: Option<String>,
}

/// `<game dir>/saves`
pub fn saves_dir(game_dir: &Path) -> PathBuf {
    game_dir.join(SAVES_DIR)
}

/// `<instance>/backups`
pub fn backups_dir(instance_dir: &Path) -> PathBuf {
    instance_dir.join(BACKUPS_DIR)
}

/// Every world in the game directory, most recently played first.
///
/// A missing `saves/` folder means the instance has simply never been played,
/// which is not an error.
pub fn worlds(game_dir: &Path) -> Result<Vec<World>> {
    let saves = saves_dir(game_dir);
    let Ok(entries) = std::fs::read_dir(&saves) else {
        return Ok(Vec::new());
    };

    let mut worlds: Vec<World> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            // The game only offers a folder as a world when it holds a level.dat,
            // so a stray folder in `saves/` is not something to back up.
            if !path.is_dir() || !path.join(LEVEL_FILE).is_file() {
                return None;
            }
            Some(World {
                name: entry.file_name().to_string_lossy().to_string(),
                size: directory_size(&path),
                modified: newest_file(&path),
                icon: path
                    .join(ICON_FILE)
                    .is_file()
                    .then(|| path.join(ICON_FILE).to_string_lossy().to_string()),
                path: path.to_string_lossy().to_string(),
            })
        })
        .collect();

    // RFC 3339 timestamps in UTC sort chronologically as strings.
    worlds.sort_by(|a, b| b.modified.cmp(&a.modified));
    Ok(worlds)
}

/// Every backup in the instance's backup folder, newest first.
pub fn list(instance_dir: &Path) -> Result<Vec<Backup>> {
    let dir = backups_dir(instance_dir);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };

    let mut backups: Vec<Backup> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() || !has_zip_extension(&path) {
                return None;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            Some(Backup {
                world: world_of(&name),
                size: std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0),
                created: modified(&path),
                path: path.to_string_lossy().to_string(),
                name,
            })
        })
        .collect();

    backups.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.name.cmp(&a.name)));
    Ok(backups)
}

/// Zip `world` into the backup folder and describe what was written.
pub fn create(game_dir: &Path, instance_dir: &Path, world: &str) -> Result<Backup> {
    let world = checked_name(world)?;
    let source = saves_dir(game_dir).join(world);
    if !source.is_dir() {
        return Err(LauncherError::msg(format!(
            "There is no world called \"{world}\" in this instance."
        )));
    }

    let dir = backups_dir(instance_dir);
    std::fs::create_dir_all(&dir)?;

    let target = unique_path(&dir, world);
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();

    // Write beside the target and rename, so an interrupted backup never leaves
    // a half-written zip that looks like a usable one.
    let partial = target.with_extension("zip.part");
    let result = zip_directory(&source, &partial);
    if let Err(error) = result {
        let _ = std::fs::remove_file(&partial);
        return Err(error);
    }
    std::fs::rename(&partial, &target)?;

    Ok(Backup {
        world: world.to_string(),
        size: std::fs::metadata(&target).map(|meta| meta.len()).unwrap_or(0),
        created: modified(&target),
        path: target.to_string_lossy().to_string(),
        name,
    })
}

/// Put a backup back in place, keeping whatever it replaces.
///
/// The world standing in the way is backed up first, so a restore can always be
/// undone — see [`Restored::safety`].
pub fn restore(game_dir: &Path, instance_dir: &Path, file: &str) -> Result<Restored> {
    let file = checked_name(file)?;
    if !file.to_ascii_lowercase().ends_with(".zip") {
        return Err(LauncherError::msg(format!("\"{file}\" is not a backup.")));
    }

    let source = backups_dir(instance_dir).join(file);
    if !source.is_file() {
        return Err(LauncherError::msg(format!(
            "There is no backup called \"{file}\"."
        )));
    }

    // The file name is the only record of which world this came from, so it has
    // to name something we are willing to write into `saves/`.
    let world = checked_name(&world_of(file))?.to_string();
    let target = saves_dir(game_dir).join(&world);

    if target.exists() && !target.is_dir() {
        return Err(LauncherError::msg(format!(
            "A file called \"{world}\" is in the way of this world."
        )));
    }

    // Zip the current world before removing it. A world that has not been played
    // yet still gets one: better an empty backup than a deleted world.
    let safety = if target.is_dir() {
        Some(create(game_dir, instance_dir, &world)?.name)
    } else {
        None
    };

    if target.is_dir() {
        // `remove_dir_all` reports a missing path as an error on Windows, which
        // is why this is spelled out rather than called unconditionally.
        std::fs::remove_dir_all(&target)?;
    }
    if let Err(error) = extract(&source, &target) {
        let hint = match &safety {
            Some(name) => format!("the world that was there is saved as \"{name}\""),
            None => "there was no world there to lose".into(),
        };
        return Err(LauncherError::msg(format!(
            "Couldn't restore \"{file}\": {error} — {hint}."
        )));
    }

    Ok(Restored { world, safety })
}

/// Delete a backup by file name.
pub fn remove(instance_dir: &Path, file: &str) -> Result<()> {
    let file = checked_name(file)?;
    let path = backups_dir(instance_dir).join(file);
    if !path.is_file() {
        return Err(LauncherError::msg(format!(
            "There is no backup called \"{file}\"."
        )));
    }
    std::fs::remove_file(&path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A world or backup name is one path segment. Anything else — a separator, a
/// parent step, an absolute path — is rejected before it can reach the disk.
fn checked_name(name: &str) -> Result<&str> {
    let name = name.trim();
    let rejected = name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', ':'])
        || name.contains('\0');
    if rejected {
        return Err(LauncherError::msg(format!(
            "\"{name}\" is not a valid world or backup name."
        )));
    }
    Ok(name)
}

/// The world a backup came from.
///
/// Names are `<world>-<stamp>.zip` and the world itself may contain dashes, so
/// the stamp is found from the right: it is the last pair of dash-separated
/// segments shaped like `YYYYMMDD` and `HHMMSS`. Anything trailing it is a
/// collision counter that `unique_path` added.
fn world_of(file_name: &str) -> String {
    let stem = file_name.strip_suffix(".zip").unwrap_or(file_name);
    let parts: Vec<&str> = stem.split('-').collect();

    // Stop at the second-to-last segment: a stamp needs a pair.
    for index in (0..parts.len().saturating_sub(1)).rev() {
        if is_stamp(parts[index], parts[index + 1]) {
            return parts[..index].join("-");
        }
    }
    stem.to_string()
}

/// Whether two dash-separated segments look like the `YYYYMMDD-HHMMSS` stamp
/// MineArcade writes, as produced by the format string in [`unique_path`].
fn is_stamp(date: &str, time: &str) -> bool {
    date.len() == 8
        && time.len() == 6
        && date.bytes().all(|byte| byte.is_ascii_digit())
        && time.bytes().all(|byte| byte.is_ascii_digit())
}

/// `<world>-<stamp>.zip`, with a counter appended if two backups of the same
/// world land in the same second.
fn unique_path(dir: &Path, world: &str) -> PathBuf {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut candidate = dir.join(format!("{world}-{stamp}.zip"));
    let mut counter = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{world}-{stamp}-{counter}.zip"));
        counter += 1;
    }
    candidate
}

fn has_zip_extension(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
}

/// Bytes on disk under `path`, following the same walk a backup takes.
fn directory_size(path: &Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| entry.metadata().ok())
        .map(|metadata| metadata.len())
        .sum()
}

/// The newest modification time under `path`, as RFC 3339.
///
/// A world folder's own mtime only moves when the game adds or removes an entry
/// at its top level, so the files inside it are what says when it was played.
fn newest_file(path: &Path) -> Option<String> {
    let newest = walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| entry.metadata().ok()?.modified().ok())
        .max()?;
    Some(chrono::DateTime::<chrono::Utc>::from(newest).to_rfc3339())
}

/// A file's modification time as RFC 3339.
fn modified(path: &Path) -> Option<String> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(chrono::DateTime::<chrono::Utc>::from(modified).to_rfc3339())
}

/// Write every file under `source` into a deflated zip at `target`.
fn zip_directory(source: &Path, target: &Path) -> Result<()> {
    let file = std::fs::File::create(target).map_err(|error| {
        LauncherError::msg(format!(
            "Couldn't create the backup file '{}': {error}",
            target.display()
        ))
    })?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    for entry in walkdir::WalkDir::new(source) {
        let entry = entry.map_err(|error| {
            LauncherError::msg(format!("Couldn't read the world folder: {error}"))
        })?;
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(source) else {
            continue;
        };
        if relative.as_os_str().is_empty() {
            continue;
        }

        // Zip entries are always forward-slashed, whatever the platform uses.
        let name = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("/");

        if entry.file_type().is_dir() {
            writer.add_directory(name.as_str(), options)?;
            continue;
        }
        if !entry.file_type().is_file() {
            // Symlinks and other oddities are skipped: the game never makes them,
            // and following one could walk out of the world.
            continue;
        }

        writer.start_file(name.as_str(), options)?;
        let mut source_file = std::fs::File::open(path).map_err(|error| {
            LauncherError::msg(format!("Couldn't read '{}': {error}", path.display()))
        })?;
        std::io::copy(&mut source_file, &mut writer)?;
    }

    writer.finish()?;
    Ok(())
}

/// Extract a backup into `target`, which must not exist.
///
/// Every entry is checked with [`zip::read::ZipFile::enclosed_name`], so a zip
/// that tries to escape its folder is skipped rather than obeyed.
fn extract(source: &Path, target: &Path) -> Result<()> {
    let file = std::fs::File::open(source).map_err(|error| {
        LauncherError::msg(format!("Couldn't open the backup: {error}"))
    })?;
    let mut archive = zip::ZipArchive::new(file)?;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        let destination = target.join(relative);

        if entry.is_dir() {
            std::fs::create_dir_all(&destination)?;
            continue;
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut output = std::fs::File::create(&destination)?;
        std::io::copy(&mut entry, &mut output)?;
    }

    // Some zips are written without directory entries at all, so an empty world
    // is still a world.
    std::fs::create_dir_all(target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A throwaway directory under the target's temp folder.
    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "minearcade-backups-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Create `saves/<name>/level.dat` (plus optional extras) under `game_dir`.
    fn make_world(game_dir: &Path, name: &str, level: &str) {
        let world = saves_dir(game_dir).join(name);
        std::fs::create_dir_all(world.join("region")).unwrap();
        std::fs::write(world.join("level.dat"), format!("level {level}")).unwrap();
        std::fs::write(world.join("region/r.0.0.mca"), vec![b'x'; 128]).unwrap();
    }

    fn read_level(world: &Path) -> String {
        std::fs::read_to_string(world.join("level.dat")).unwrap()
    }

    #[test]
    fn lists_worlds_and_ignores_folders_that_are_not_ones() {
        let root = temp_dir("list");
        let game = root.join("game");
        let instance = root.join("instance");
        make_world(&game, "Survival", "one");
        make_world(&game, "Creative World", "two");
        // The game draws an icon once a world has been played.
        std::fs::write(
            saves_dir(&game).join("Survival").join(ICON_FILE),
            b"png",
        )
        .unwrap();
        // Not a world: the game would not list it either.
        std::fs::create_dir_all(saves_dir(&game).join("empty")).unwrap();
        // A file in saves/ is ignored as well.
        std::fs::write(saves_dir(&game).join("note.txt"), "hi").unwrap();

        let listed = worlds(&game).unwrap();
        let names: Vec<&str> = listed.iter().map(|world| world.name.as_str()).collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"Survival"));
        assert!(names.contains(&"Creative World"));
        assert!(listed.iter().all(|world| world.size > 128));
        let survival = listed
            .iter()
            .find(|world| world.name == "Survival")
            .expect("Survival");
        assert!(survival.icon.is_some(), "icon.png was not reported");
        assert!(survival.modified.is_some());
        // A world the game has not drawn one for reports no icon.
        assert!(listed
            .iter()
            .find(|world| world.name == "Creative World")
            .expect("Creative World")
            .icon
            .is_none());

        // An instance with no saves folder simply has no worlds.
        assert!(worlds(&instance.join("nowhere")).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn backs_up_a_world_and_reads_it_back() {
        let root = temp_dir("create");
        let game = root.join("game");
        let instance = root.join("instance");
        make_world(&game, "Survival", "before");

        let backup = create(&game, &instance, "Survival").unwrap();
        assert_eq!(backup.world, "Survival");
        assert!(backup.name.starts_with("Survival-"));
        assert!(backup.name.ends_with(".zip"));
        assert!(backup.size > 0);
        assert!(backup.created.is_some());
        assert!(Path::new(&backup.path).is_file());

        // No `.part` file survives a successful backup.
        let leftovers: Vec<PathBuf> = std::fs::read_dir(backups_dir(&instance))
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "part"))
            .collect();
        assert!(leftovers.is_empty(), "left behind {leftovers:?}");

        let listed = list(&instance).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, backup.name);
        assert_eq!(listed[0].world, "Survival");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_second_backup_in_the_same_second_does_not_clobber_the_first() {
        let root = temp_dir("collide");
        let game = root.join("game");
        let instance = root.join("instance");
        make_world(&game, "Survival", "one");

        let first = create(&game, &instance, "Survival").unwrap();
        let second = create(&game, &instance, "Survival").unwrap();
        assert_ne!(first.name, second.name);
        // Both still belong to the world, counter and all.
        assert_eq!(world_of(&second.name), "Survival");
        assert_eq!(list(&instance).unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn restoring_replaces_the_world_and_keeps_the_old_one() {
        let root = temp_dir("restore");
        let game = root.join("game");
        let instance = root.join("instance");
        make_world(&game, "Survival", "before");

        let backup = create(&game, &instance, "Survival").unwrap();
        // Play on: the world now holds different data.
        make_world(&game, "Survival", "after");
        assert_eq!(read_level(&saves_dir(&game).join("Survival")), "level after");

        let restored = restore(&game, &instance, &backup.name).unwrap();
        assert_eq!(restored.world, "Survival");
        assert_eq!(read_level(&saves_dir(&game).join("Survival")), "level before");

        // The world it replaced is on disk, not gone.
        let safety = restored.safety.expect("a safety backup");
        assert!(backups_dir(&instance).join(&safety).is_file());
        assert!(world_of(&safety) == "Survival");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn restoring_into_an_empty_saves_folder_takes_no_safety_copy() {
        let root = temp_dir("fresh");
        let game = root.join("game");
        let instance = root.join("instance");
        make_world(&game, "Survival", "saved");
        let backup = create(&game, &instance, "Survival").unwrap();

        std::fs::remove_dir_all(saves_dir(&game)).unwrap();
        let restored = restore(&game, &instance, &backup.name).unwrap();
        assert_eq!(restored.safety, None);
        assert_eq!(read_level(&saves_dir(&game).join("Survival")), "level saved");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn world_names_cannot_escape_the_saves_folder() {
        let root = temp_dir("escape");
        let game = root.join("game");
        let instance = root.join("instance");
        make_world(&game, "Survival", "one");

        for bad in ["..", "../evil", "saves/../x", r"..\evil", "", " ", "C:", "a/b"] {
            assert!(create(&game, &instance, bad).is_err(), "{bad:?} was accepted");
        }
        // Nothing was written outside the backup folder.
        assert!(!instance.exists() || list(&instance).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_restore_only_accepts_backups_in_the_backup_folder() {
        let root = temp_dir("outside");
        let game = root.join("game");
        let instance = root.join("instance");
        make_world(&game, "Survival", "one");
        let backup = create(&game, &instance, "Survival").unwrap();

        assert!(restore(&game, &instance, "../evil.zip").is_err());
        assert!(restore(&game, &instance, "nope.zip").is_err());
        assert!(restore(&game, &instance, "level.dat").is_err());
        assert!(remove(&instance, "nope.zip").is_err());

        remove(&instance, &backup.name).unwrap();
        assert!(list(&instance).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_zip_that_writes_outside_its_folder_is_ignored() {
        let root = temp_dir("zip-slip");
        let instance = root.join("instance");
        let game = root.join("game");
        std::fs::create_dir_all(backups_dir(&instance)).unwrap();

        // Hand-build a zip whose entry tries to climb out of the world folder.
        let target = backups_dir(&instance).join("Evil-20250101-000000.zip");
        let file = std::fs::File::create(&target).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("../escaped.txt", options).unwrap();
        writer.write_all(b"nope").unwrap();
        writer.start_file("level.dat", options).unwrap();
        writer.write_all(b"level evil").unwrap();
        writer.finish().unwrap();

        let restored = restore(&game, &instance, "Evil-20250101-000000.zip").unwrap();
        assert_eq!(restored.world, "Evil");
        assert_eq!(read_level(&saves_dir(&game).join("Evil")), "level evil");
        assert!(!root.join("escaped.txt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn names_with_dashes_round_trip_through_the_file_name() {
        assert_eq!(world_of("Survival-20251018-120000.zip"), "Survival");
        assert_eq!(world_of("My-World-20251018-120000.zip"), "My-World");
        assert_eq!(world_of("My-World-20251018-120000-2.zip"), "My-World");
        // Nothing stamp-shaped to strip: the whole stem is the name.
        assert_eq!(world_of("Survival.zip"), "Survival");
        assert_eq!(world_of("2025-2026.zip"), "2025-2026");
    }
}
