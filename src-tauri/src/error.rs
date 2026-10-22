//! Unified error type for the whole launcher.
//!
//! Every fallible operation returns [`Result`], and [`LauncherError`] implements
//! `Serialize` so it can be returned directly from a Tauri command and rendered
//! as a friendly message in the UI.

use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum LauncherError {
    /// A generic, already user-friendly message.
    #[error("{0}")]
    Message(String),

    #[error("We couldn't reach the network: {0}")]
    Network(#[from] reqwest::Error),

    #[error("A file operation failed: {0}")]
    Io(#[from] std::io::Error),

    #[error("We couldn't read the launcher data: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("Couldn't find {0}")]
    NotFound(String),

    #[error("Authentication failed: {0}")]
    Auth(String),

    #[error("Couldn't start the game: {0}")]
    Launch(String),

    #[error("This isn't supported yet: {0}")]
    Unsupported(String),

    #[error("Archive error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("Invalid data: {0}")]
    InvalidData(String),

    #[error("Rate limit exceeded: {0}")]
    RateLimited(String),

    #[error("Minecraft API error: {0}")]
    Minecraft(String),

    /// Reading another launcher's SQLite database (the Modrinth App).
    #[error("We couldn't read another launcher's data: {0}")]
    Database(#[from] rusqlite::Error),
}

impl LauncherError {
    pub fn msg(message: impl Into<String>) -> Self {
        LauncherError::Message(message.into())
    }
}

/// Serialise errors as plain strings so the frontend can show them verbatim.
impl Serialize for LauncherError {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, LauncherError>;
