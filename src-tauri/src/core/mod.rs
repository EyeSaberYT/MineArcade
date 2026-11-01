//! Core launcher logic, free of any Tauri types.
//!
//! Keeping this layer framework-agnostic means it can be unit tested and
//! reused (e.g. from a future CLI) without dragging in the GUI.

/// Start child processes without a console window. On Windows, every console
/// program the (windowed) launcher runs - `java -version` probes, loader
/// installers, the game itself - otherwise flashes up its own command prompt.
pub fn no_console(command: &mut tokio::process::Command) -> &mut tokio::process::Command {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

pub mod auth;
pub mod backups;
pub mod cfpack;
pub mod config;
pub mod discord;
pub mod download;
pub mod import;
pub mod instance;
pub mod java;
pub mod launch;
pub mod minecraft;
pub mod modpack;
pub mod paths;
pub mod path_security;
pub mod scan;
pub mod screenshots;
pub mod servers;
pub mod skin;
