//! Filesystem layout used by Orbit Launcher.
//!
//! Everything the launcher owns lives under a single root directory so that a
//! user can back it up, move it, or delete it trivially.
//!
//! ```text
//! <root>/
//!   config.json          global launcher settings
//!   accounts.json        cached Microsoft/offline accounts & tokens
//!   instances.json       instance metadata index
//!   instances/<id>/
//!     instance.json      per-instance settings + resolved loaders
//!     (game files)       the game directory — mods/, logs/, saves/, …
//!   meta/                cached Mojang metadata (version manifest, etc.)
//!   java/                launcher-managed Java runtimes
//!   cache/               downloads, icons & asset cache
//! ```

use std::path::{Path, PathBuf};

use crate::error::Result;

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub root: PathBuf,
}

impl AppPaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `<root>/config.json`
    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.json")
    }

    /// `<root>/accounts.json`
    pub fn accounts_file(&self) -> PathBuf {
        self.root.join("accounts.json")
    }

    /// `<root>/instances.json`
    pub fn instances_index(&self) -> PathBuf {
        self.root.join("instances.json")
    }

    /// `<root>/instances`
    pub fn instances_dir(&self) -> PathBuf {
        self.root.join("instances")
    }

    /// `<root>/instances/<id>`
    pub fn instance_dir(&self, id: &str) -> PathBuf {
        self.instances_dir().join(id)
    }

    /// `<root>/instances/<id>/instance.json`
    pub fn instance_file(&self, id: &str) -> PathBuf {
        self.instance_dir(id).join("instance.json")
    }

    /// `<root>/instances/<id>` — the instance folder *is* the game directory.
    ///
    /// There is no nested `.minecraft/`: `mods/`, `logs/`, `saves/` and the
    /// rest sit right next to `instance.json`, which keeps every instance
    /// self-contained and easy to find by hand.
    pub fn instance_game_dir(&self, id: &str) -> PathBuf {
        self.instance_dir(id)
    }

    /// `<root>/meta`
    pub fn meta_dir(&self) -> PathBuf {
        self.root.join("meta")
    }

    /// `<root>/meta/versions`
    pub fn versions_dir(&self) -> PathBuf {
        self.meta_dir().join("versions")
    }

    /// `<root>/meta/assets`
    pub fn assets_dir(&self) -> PathBuf {
        self.meta_dir().join("assets")
    }

    /// `<root>/meta/libraries`
    pub fn libraries_dir(&self) -> PathBuf {
        self.meta_dir().join("libraries")
    }

    /// `<root>/java`
    pub fn java_dir(&self) -> PathBuf {
        self.root.join("java")
    }

    /// `<root>/cache`
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// `<root>/cache/icons`
    pub fn icon_cache_dir(&self) -> PathBuf {
        self.cache_dir().join("icons")
    }

    /// Create every directory the launcher relies on.
    pub fn ensure(&self) -> Result<()> {
        for dir in [
            self.root.clone(),
            self.instances_dir(),
            self.meta_dir(),
            self.versions_dir(),
            self.assets_dir(),
            self.assets_dir().join("indexes"),
            self.assets_dir().join("objects"),
            self.libraries_dir(),
            self.java_dir(),
            self.cache_dir(),
            self.icon_cache_dir(),
        ] {
            std::fs::create_dir_all(&dir)?;
        }
        Ok(())
    }

    /// Resolve the correct place for the launcher data on the host OS.
    ///
    /// Everything lives in one dot-prefixed folder (`.orbitlauncher`) so it is
    /// easy to spot, back up or delete. An explicit `ORBIT_LAUNCHER_DATA_DIR`
    /// (or the old `MINEARCADE_DATA_DIR`) always wins.
    pub fn discover() -> Self {
        let override_dir =
            std::env::var("ORBIT_LAUNCHER_DATA_DIR").or_else(|_| std::env::var("MINEARCADE_DATA_DIR"));
        let paths = if let Ok(override_dir) = override_dir {
            Self::new(override_dir)
        } else {
            let data_dir = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
            Self::new(migrate_legacy_root(&data_dir))
        };
        // Layout normalisation applies to every root, overrides included: it
        // only rearranges instance folders Orbit Launcher itself wrote.
        flatten_instances(&paths.root);
        paths
    }
}

/// Data folder name used since the rename to Orbit Launcher.
const ROOT_DIR_NAME: &str = ".orbitlauncher";
/// Data folder name from before the rename to Orbit Launcher.
const LEGACY_ROOT_DIR_NAME: &str = ".minearcade";

/// Pick the data root under `data_dir`, moving the pre-rename folder over once.
///
/// When only the legacy folder exists it is renamed to the new name. If that
/// rename fails (another volume, a file in use, ...) the legacy folder keeps
/// being used as-is, so an install never starts over with an empty library.
fn migrate_legacy_root(data_dir: &Path) -> PathBuf {
    let root = data_dir.join(ROOT_DIR_NAME);
    let legacy = data_dir.join(LEGACY_ROOT_DIR_NAME);
    if root.exists() || !legacy.is_dir() {
        return root;
    }
    match std::fs::rename(&legacy, &root) {
        Ok(()) => root,
        Err(_) => legacy,
    }
}

/// Move every entry of `from` into `to`, recursing where both sides already
/// have the same directory.
fn merge_dir(from: &Path, to: &Path) {
    let Ok(entries) = std::fs::read_dir(from) else {
        return;
    };
    if std::fs::create_dir_all(to).is_err() {
        return;
    }

    for entry in entries.flatten() {
        let src = entry.path();
        let dst = to.join(entry.file_name());

        if src.is_dir() {
            if dst.is_dir() {
                merge_dir(&src, &dst);
                let _ = std::fs::remove_dir(&src);
            } else if !dst.exists() {
                let _ = std::fs::rename(&src, &dst);
            }
        } else if !dst.exists() {
            // A rename can fail across volumes; fall back to copy + delete.
            if std::fs::rename(&src, &dst).is_err() && std::fs::copy(&src, &dst).is_ok() {
                let _ = std::fs::remove_file(&src);
            }
        }
    }
}

/// Flatten instances written before 0.1.1, which kept their game files in a
/// nested `.minecraft/` folder. Every entry moves up into the instance folder
/// itself, which is now the game directory (`instances/<id>/mods`, `/logs`, …).
///
/// Best effort: whatever is still in the way is left
/// behind rather than destroyed, and the launcher runs off what it can see.
fn flatten_instances(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root.join("instances")) else {
        return;
    };

    for entry in entries.flatten() {
        let instance = entry.path();
        let nested = instance.join(".minecraft");
        if !nested.is_dir() {
            continue;
        }
        merge_dir(&nested, &instance);
        // Only succeeds once every entry has been moved out.
        let _ = std::fs::remove_dir(&nested);
    }
}

/// Read JSON from a file, returning `None` when the file does not exist.
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path)?;
    if raw.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&raw)?))
}

/// Write JSON to a file atomically (write to a temp file then rename).
pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let pretty = serde_json::to_string_pretty(value)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, pretty)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clean scratch directory under the temp folder, keyed by test name so
    /// the tests can run in parallel.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbit-launcher-paths-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn flattens_nested_game_folders_into_the_instance() {
        let base = scratch("flatten");
        let clean = base.join("instances/abc123");
        let conflicted = base.join("instances/def456");

        // A pre-0.1.1 instance: everything under a nested `.minecraft/`.
        std::fs::create_dir_all(clean.join(".minecraft/mods")).expect("nested dirs");
        std::fs::write(clean.join(".minecraft/mods/sodium.jar"), "jar").expect("mod");
        std::fs::write(clean.join(".minecraft/options.txt"), "fov:0.0\n").expect("options");
        std::fs::write(clean.join("instance.json"), "{}").expect("meta");

        // An instance that already has flat content the nested copy collides
        // with: the existing file wins and the loser is kept, not destroyed.
        std::fs::create_dir_all(conflicted.join(".minecraft/logs")).expect("nested logs");
        std::fs::create_dir_all(conflicted.join("logs")).expect("flat logs");
        std::fs::write(conflicted.join(".minecraft/logs/latest.log"), "old").expect("nested log");
        std::fs::write(conflicted.join("logs/latest.log"), "new").expect("flat log");

        flatten_instances(&base);

        assert!(
            clean.join("mods/sodium.jar").is_file(),
            "game folders must move up into the instance folder"
        );
        assert!(clean.join("options.txt").is_file(), "loose files must move up");
        assert!(clean.join("instance.json").is_file(), "launcher metadata must stay");
        assert!(!clean.join(".minecraft").exists(), "the nested folder must be gone");

        assert_eq!(
            std::fs::read_to_string(conflicted.join("logs/latest.log")).unwrap(),
            "new",
            "existing flat files win over the nested copies"
        );
        assert!(
            conflicted.join(".minecraft/logs/latest.log").is_file(),
            "unmovable stragglers must be kept rather than deleted"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn moves_the_legacy_data_folder_once() {
        let base = scratch("migrate");
        let legacy = base.join(LEGACY_ROOT_DIR_NAME);
        std::fs::create_dir_all(legacy.join("instances/abc123")).expect("legacy dirs");
        std::fs::write(legacy.join("config.json"), "{}").expect("legacy config");

        // Only the legacy folder: it is moved to the new name.
        let root = migrate_legacy_root(&base);
        assert_eq!(root, base.join(ROOT_DIR_NAME));
        assert!(root.join("config.json").is_file(), "settings must move along");
        assert!(root.join("instances/abc123").is_dir(), "instances must move along");
        assert!(!legacy.exists(), "the legacy folder must be gone after the move");

        // A legacy folder reappearing next to the new one is left untouched.
        std::fs::create_dir_all(&legacy).expect("stray legacy dir");
        std::fs::write(legacy.join("config.json"), "stray").expect("stray config");
        assert_eq!(migrate_legacy_root(&base), base.join(ROOT_DIR_NAME));
        assert_eq!(
            std::fs::read_to_string(root.join("config.json")).unwrap(),
            "{}",
            "an existing new folder must never be overwritten"
        );
        assert!(legacy.join("config.json").is_file(), "the stray folder must be kept, not deleted");

        // Neither folder: a fresh install uses the new name.
        let fresh = scratch("migrate-fresh");
        assert_eq!(migrate_legacy_root(&fresh), fresh.join(ROOT_DIR_NAME));
        assert!(!fresh.join(ROOT_DIR_NAME).exists(), "nothing is created up front");

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&fresh);
    }
}
