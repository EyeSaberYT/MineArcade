mod auth;
mod commands;
mod config;
mod download;
mod fabric;
mod java;
mod launch;
mod paths;
mod progress;
mod state;
mod versions;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = AppState::new().expect("could not initialise the MineArcade data directory");

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::get_settings,
            commands::save_settings,
            commands::list_instances,
            commands::create_instance,
            commands::update_instance,
            commands::delete_instance,
            commands::open_instance_folder,
            commands::open_data_folder,
            commands::open_url,
            commands::get_versions,
            commands::fabric_versions,
            commands::list_accounts,
            commands::login_start,
            commands::login_wait,
            commands::login_cancel,
            commands::set_active_account,
            commands::remove_account,
            commands::launch_instance,
            commands::kill_instance,
            commands::running_instances,
        ])
        .run(tauri::generate_context!())
        .expect("error while running MineArcade");
}
