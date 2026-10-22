//! In-game screenshots: the PNGs the game drops in `<game dir>/screenshots`.
//!
//! Read-only apart from deleting a single shot. Nothing here invents files: the
//! folder belongs to the game, and the gallery is just a window onto it.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{LauncherError, Result};

/// Folder the game writes its screenshots to, relative to a game directory.
pub const SCREENSHOTS_DIR: &str = "screenshots";

/// One screenshot in the game's `screenshots/`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Screenshot {
    /// File name. Deleting refers to a screenshot by this, never by a path.
    pub name: String,
    /// Absolute path, so the webview can draw it with an `asset:` URL.
    pub path: String,
    pub size: u64,
    /// When the file was written, which is when the shot was taken.
    pub taken: Option<String>,
}

/// `<game dir>/screenshots`
pub fn dir(game_dir: &Path) -> PathBuf {
    game_dir.join(SCREENSHOTS_DIR)
}

/// Every screenshot in the game directory, newest first.
///
/// A missing `screenshots/` folder means the instance has never had one, which
/// is not an error. Only images are listed, so whatever else ends up in there is
/// left alone.
pub fn list(game_dir: &Path) -> Result<Vec<Screenshot>> {
    let dir = dir(game_dir);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };

    let mut screenshots: Vec<Screenshot> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() || !is_image(&path) {
                return None;
            }
            Some(Screenshot {
                name: entry.file_name().to_string_lossy().to_string(),
                size: std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0),
                taken: modified(&path),
                path: path.to_string_lossy().to_string(),
            })
        })
        .collect();

    screenshots.sort_by(|a, b| b.taken.cmp(&a.taken).then_with(|| b.name.cmp(&a.name)));
    Ok(screenshots)
}

/// Delete one screenshot by file name.
pub fn remove(game_dir: &Path, name: &str) -> Result<()> {
    let name = checked_name(name)?;
    let path = dir(game_dir).join(name);

    // Only a real image that is really there can be deleted: the name arrived
    // from the frontend, and deleting is not undoable.
    if !path.is_file() || !is_image(&path) {
        return Err(LauncherError::msg(format!(
            "There is no screenshot called \"{name}\"."
        )));
    }
    std::fs::remove_file(&path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A screenshot name is one path segment, and it is an image.
fn checked_name(name: &str) -> Result<&str> {
    let name = name.trim();
    let rejected = name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', ':'])
        || name.contains('\0');
    if rejected {
        return Err(LauncherError::msg(format!(
            "\"{name}\" is not a valid screenshot name."
        )));
    }
    Ok(name)
}

/// The game writes PNGs, and only PNGs, but a couple of formats are accepted so
/// a converted shot someone dropped in still shows up.
fn is_image(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        ["png", "jpg", "jpeg"]
            .iter()
            .any(|known| extension.eq_ignore_ascii_case(known))
    })
}

/// A file's modification time as RFC 3339.
fn modified(path: &Path) -> Option<String> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(chrono::DateTime::<chrono::Utc>::from(modified).to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "minearcade-shots-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lists_only_images_and_survives_a_missing_folder() {
        let root = temp_dir("list");
        let game = root.join("game");
        std::fs::create_dir_all(dir(&game)).unwrap();
        std::fs::write(dir(&game).join("2025-10-18_12.00.00.png"), b"png").unwrap();
        std::fs::write(dir(&game).join("2025-10-18_12.00.01.PNG"), b"png").unwrap();
        std::fs::write(dir(&game).join("notes.txt"), b"text").unwrap();
        std::fs::create_dir_all(dir(&game).join("subfolder")).unwrap();

        let shots = list(&game).unwrap();
        let names: Vec<&str> = shots.iter().map(|shot| shot.name.as_str()).collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"2025-10-18_12.00.00.png"));
        assert!(names.contains(&"2025-10-18_12.00.01.PNG"));
        assert!(shots.iter().all(|shot| shot.taken.is_some()));

        // An instance that has never taken a screenshot has no folder at all.
        assert!(list(&root.join("nowhere")).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn deletes_one_shot_and_refuses_everything_else() {
        let root = temp_dir("delete");
        let game = root.join("game");
        std::fs::create_dir_all(dir(&game)).unwrap();
        std::fs::write(dir(&game).join("shot.png"), b"png").unwrap();
        std::fs::write(dir(&game).join("notes.txt"), b"text").unwrap();

        assert!(remove(&game, "../shot.png").is_err());
        assert!(remove(&game, "notes.txt").is_err());
        assert!(remove(&game, "missing.png").is_err());
        assert!(remove(&game, "").is_err());

        remove(&game, "shot.png").unwrap();
        assert!(list(&game).unwrap().is_empty());
        // The unrelated file is still there.
        assert!(dir(&game).join("notes.txt").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }
}
