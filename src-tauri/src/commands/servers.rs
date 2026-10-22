//! Reading and writing an instance's multiplayer server list (`servers.dat`).

use std::path::Path;

use tauri::State;

use crate::core::instance::game_dir;
use crate::core::servers::{self, ServerEntry, ServerList};
use crate::error::Result;
use crate::state::AppState;

/// An instance's server list, plus enough context for the UI to explain itself.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerListFile {
    /// Absolute path to the list, so the UI can offer to reveal it.
    pub path: String,
    /// `false` when this instance has never had a multiplayer list.
    pub exists: bool,
    pub servers: Vec<ServerEntry>,
}

impl ServerListFile {
    fn new(path: &Path, exists: bool, list: ServerList) -> Self {
        Self {
            path: path.display().to_string(),
            exists,
            servers: list.servers,
        }
    }
}

/// The instance's multiplayer server list, as the game would see it.
#[tauri::command]
pub async fn servers_list(state: State<'_, AppState>, id: String) -> Result<ServerListFile> {
    let instance = state.instances.read().await.get(&id)?.clone();
    let path = ServerList::path_in(&game_dir(&state.paths, &instance));

    match servers::read(&path)? {
        Some(list) => Ok(ServerListFile::new(&path, true, list)),
        // An instance that has never been in the multiplayer screen simply has
        // no list yet; that is not an error.
        None => Ok(ServerListFile::new(&path, false, ServerList::default())),
    }
}

/// Replace the instance's multiplayer server list.
///
/// The whole list is written at once: the UI edits it as a unit and the game
/// only reads the file at startup, so there is nothing to merge with. The game
/// writes this same file back whenever the player edits the list in game, which
/// is why the UI warns instead of MineArcade quietly overwriting a running instance.
#[tauri::command]
pub async fn servers_save(
    state: State<'_, AppState>,
    id: String,
    servers: Vec<ServerEntry>,
) -> Result<ServerListFile> {
    let instance = state.instances.read().await.get(&id)?.clone();
    let path = ServerList::path_in(&game_dir(&state.paths, &instance));
    let list = ServerList { servers };

    servers::write(&path, &list)?;
    Ok(ServerListFile::new(&path, true, list))
}
