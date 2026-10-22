//! MineArcade — the MineArcade network's Minecraft launcher.
//!
//! The crate is split into three layers:
//!
//! * [`core`] – pure launcher logic (auth, instances, Minecraft, Java,
//!   downloads). No Tauri types, fully testable.
//! * [`api`] – remote service clients (Modrinth/Labrinth).
//! * [`commands`] – the thin Tauri command surface the React frontend talks to.

pub mod api;
pub mod commands;
pub mod core;
pub mod error;
pub mod state;

use tauri::Manager;

use crate::core::paths::AppPaths;

/// Sent on every HTTP request so Modrinth/Mojang can identify the launcher.
pub const USER_AGENT: &str = concat!("MineArcade/", env!("CARGO_PKG_VERSION"), " (minearcade.org)");

/// Entry point used by both `main.rs` and mobile targets.
pub fn run() {
    let paths = AppPaths::discover();

    // Structured logs help users file useful bug reports.
    let log_dir = paths.root.join("logs");
    let _ = std::fs::create_dir_all(&log_dir);

    let state = match state::AppState::new(paths) {
        Ok(state) => state,
        Err(error) => {
            eprintln!("MineArcade failed to start: {error}");
            std::process::exit(1);
        }
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        // Self-update: downloads signed releases published on GitHub, and
        // restarts into them via the process plugin.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(state)
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title("MineArcade");
            }
            // Screenshots and world icons are drawn with `asset:` URLs, and the
            // data root is only known at runtime (`MINEARCADE_DATA_DIR` can put it
            // anywhere), so the scope is granted here instead of being written
            // into the bundle config.
            let root = app.state::<state::AppState>().paths.root.clone();
            app.asset_protocol_scope().allow_directory(&root, true)?;
            log::info!(
                "MineArcade {} starting on {} ({})",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH
            );
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // ---- auth ------------------------------------------------------
            commands::auth::auth_accounts,
            commands::auth::auth_current,
            commands::auth::auth_login_microsoft,
            commands::auth::auth_logout,
            commands::auth::auth_set_active,
            // ---- instances -------------------------------------------------
            commands::instance::instance_list,
            commands::instance::instance_get,
            commands::instance::instance_create,
            commands::instance::instance_update,
            commands::instance::instance_rename,
            commands::instance::instance_duplicate,
            commands::instance::instance_delete,
            commands::instance::instance_install_loader,
            commands::instance::instance_content,
            commands::instance::instance_toggle_content,
            commands::instance::instance_delete_content,
            commands::instance::instance_import_files,
            commands::instance::instance_paths,
            commands::instance::instance_content_dir_name,
            commands::instance::instance_default_settings,
            commands::instance::instance_slug,
            commands::instance::modpack_preview,
            commands::instance::modpack_import,
            commands::instance::modpack_export,
            // ---- versions --------------------------------------------------
            commands::versions::versions_list,
            commands::versions::versions_refresh,
            commands::versions::versions_latest,
            commands::versions::loader_versions,
            commands::versions::version_details,
            // ---- launch ----------------------------------------------------
            commands::launch::launch_instance,
            commands::launch::launch_preview,
            commands::launch::launch_logs,
            commands::launch::launch_mark_stopped,
            commands::launch::launch_running,
            // ---- modrinth --------------------------------------------------
            commands::modrinth::modrinth_search,
            commands::modrinth::modrinth_project,
            commands::modrinth::modrinth_projects,
            commands::modrinth::modrinth_versions,
            commands::modrinth::modrinth_tags,
            commands::modrinth::modrinth_install,
            commands::modrinth::modrinth_install_modpack,
            commands::modrinth::modrinth_check_updates,
            commands::modrinth::modrinth_apply_update,
            commands::modrinth::modrinth_update_all,
            commands::modrinth::modrinth_dependencies,

            commands::scan::scan_instance,

            // ---- multiplayer server list -----------------------------------
            commands::servers::servers_list,
            commands::servers::servers_save,
            // ---- worlds and screenshots ------------------------------------
            commands::backups::worlds_overview,
            commands::backups::world_backup_create,
            commands::backups::world_backup_restore,
            commands::backups::world_backup_delete,
            commands::screenshots::screenshots_list,
            commands::screenshots::screenshot_delete,
            // ---- import -----------------------------------------------------
            commands::import::import_scan,
            commands::import::import_inspect,
            commands::import::import_instances,
            // ---- java ------------------------------------------------------
            commands::java::java_list,
            commands::java::java_detect,
            commands::java::java_install,
            commands::java::java_requirement,
            // ---- system ----------------------------------------------------
            commands::system::settings_get,
            commands::system::settings_save,
            commands::system::app_info,
            commands::system::system_reveal,
            commands::system::system_open_url,
            commands::system::system_free_space,
            // ---- skin -------------------------------------------------------
            commands::skin::skin_current,
            commands::skin::skin_upload,
        ])
        .run(tauri::generate_context!())
        .expect("error while running MineArcade");
}

/// Convenience for tests and future CLI entry points.
pub fn paths() -> AppPaths {
    AppPaths::discover()
}

/// Re-export for integration tests.
pub use state::AppState as LauncherState;
pub type SharedState = state::AppState;

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// The updater reads `plugins.updater` from `tauri.conf.json` through a
    /// serde `Deserialize` that ignores unknown fields, so a mistyped key
    /// would silently turn off the protection it is meant to provide. This
    /// pins the config against the plugin's own `Config` type — a typo, a
    /// dropped line, or a plugin upgrade renaming the field all fail here
    /// instead of shipping a launcher that accepts downgrades.
    #[test]
    fn updater_config_enforces_signed_versions() {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
        let text = std::fs::read_to_string(&file).expect("read tauri.conf.json");
        let conf: serde_json::Value = serde_json::from_str(&text).expect("parse tauri.conf.json");
        let updater = conf
            .pointer("/plugins/updater")
            .expect("tauri.conf.json has plugins.updater");

        let updater: tauri_plugin_updater::Config =
            serde_json::from_value(updater.clone()).expect("updater config deserialises");

        assert!(
            updater.require_signed_version,
            "requireSignedVersion must stay on: the update manifest is served over TLS but is \
             itself unsigned, so only this check binds an announced version to a signed artifact"
        );
        assert!(!updater.pubkey.is_empty(), "an updater pubkey is required");
        assert!(
            updater.endpoints.iter().all(|url| url.scheme() == "https"),
            "update endpoints must be https"
        );
    }
}
