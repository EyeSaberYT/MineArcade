//! Small path checks shared by command handlers and archive installers.

use std::path::{Component, Path};

/// Accept a filename, not a path. Check both separator styles because metadata
/// can be received on one OS and installed on another.
pub fn is_safe_filename(name: &str) -> bool {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.chars().any(|c| matches!(c, '/' | '\\' | ':'))
        || name.chars().any(char::is_control)
    {
        return false;
    }

    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

/// Reject archive paths that are absolute, contain traversal, or use Windows
/// path syntax. Zip entries conventionally use forward slashes.
pub fn is_safe_archive_path(relative: &str) -> bool {
    !relative.is_empty()
        && !relative.starts_with('/')
        && !relative.starts_with('\\')
        && !relative.chars().any(|c| c == '\\' || c == ':' || c.is_control())
        && relative
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

/// Confirm that a destination's parent and, if present, the destination
/// itself still resolve beneath the root after directories have been created.
pub fn destination_is_within(root: &Path, destination: &Path) -> bool {
    let Some(parent) = destination.parent() else {
        return false;
    };
    canonical_path_is_within(root, parent)
        && (!destination.exists() || canonical_path_is_within(root, destination))
}

/// True only when an existing path resolves inside an existing root. This
/// follows symlinks and junctions, unlike lexical `Path::starts_with` checks.
pub fn canonical_path_is_within(root: &Path, candidate: &Path) -> bool {
    match (root.canonicalize(), candidate.canonicalize()) {
        (Ok(root), Ok(candidate)) => candidate.starts_with(root),
        _ => false,
    }
}
