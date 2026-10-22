use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::auth::{self, DeviceCode};
use crate::config::{self, AccountView, Instance, Loader, Settings};
use crate::fabric::{self, LoaderVersion};
use crate::launch;
use crate::state::AppState;
use crate::versions::{self, VersionManifest};

type CmdResult<T> = Result<T, String>;

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

fn io_err(e: std::io::Error) -> String {
    e.to_string()
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn clamp_memory(mb: u32) -> u32 {
    mb.clamp(512, 65536)
}

/// Instance ids become folder names, so only accept the shape we generate.
fn is_safe_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

// ---------- App & settings ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: &'static str,
    data_dir: String,
    built_in_client_id: bool,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        data_dir: state.paths.root.to_string_lossy().into_owned(),
        built_in_client_id: option_env!("MINEARCADE_CLIENT_ID").is_some_and(|id| !id.trim().is_empty()),
    }
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    config::load_settings(&state.paths)
}

#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: Settings) -> CmdResult<()> {
    let settings = Settings {
        client_id: settings.client_id.trim().to_string(),
        featured_server: settings.featured_server.trim().to_string(),
        default_memory_mb: clamp_memory(settings.default_memory_mb),
        ..settings
    };
    config::save_settings(&state.paths, &settings).map_err(err)
}

#[tauri::command]
pub fn open_data_folder(state: State<'_, AppState>) -> CmdResult<()> {
    open::that_detached(&state.paths.root).map_err(io_err)
}

#[tauri::command]
pub fn open_url(url: String) -> CmdResult<()> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Only web links can be opened.".into());
    }
    open::that_detached(&url).map_err(io_err)
}

// ---------- Instances ----------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceDraft {
    name: String,
    mc_version: String,
    loader: Loader,
    #[serde(default)]
    loader_version: Option<String>,
    #[serde(default)]
    memory_mb: Option<u32>,
    #[serde(default)]
    auto_join: Option<String>,
    #[serde(default)]
    jvm_args: Option<String>,
    #[serde(default)]
    java_path: Option<String>,
}

#[tauri::command]
pub fn list_instances(state: State<'_, AppState>) -> Vec<Instance> {
    config::load_instances(&state.paths)
}

#[tauri::command]
pub fn create_instance(state: State<'_, AppState>, draft: InstanceDraft) -> CmdResult<Instance> {
    let name = draft.name.trim();
    if name.is_empty() {
        return Err("Give the instance a name.".into());
    }
    if draft.mc_version.trim().is_empty() {
        return Err("Pick a Minecraft version.".into());
    }

    let settings = config::load_settings(&state.paths);
    let instance = Instance {
        id: uuid::Uuid::new_v4().simple().to_string(),
        name: name.to_string(),
        mc_version: draft.mc_version.trim().to_string(),
        loader: draft.loader,
        loader_version: if draft.loader == Loader::Fabric { non_empty(draft.loader_version) } else { None },
        memory_mb: clamp_memory(draft.memory_mb.unwrap_or(settings.default_memory_mb)),
        jvm_args: draft.jvm_args.unwrap_or_default().trim().to_string(),
        java_path: non_empty(draft.java_path),
        auto_join: non_empty(draft.auto_join),
        created_at: config::now_secs(),
        last_played: None,
    };

    std::fs::create_dir_all(state.paths.instance_dir(&instance.id)).map_err(io_err)?;
    let mut instances = config::load_instances(&state.paths);
    instances.push(instance.clone());
    config::save_instances(&state.paths, &instances).map_err(err)?;
    Ok(instance)
}

#[tauri::command]
pub fn update_instance(state: State<'_, AppState>, instance: Instance) -> CmdResult<()> {
    let name = instance.name.trim().to_string();
    if name.is_empty() {
        return Err("Give the instance a name.".into());
    }
    config::update_instance(&state.paths, &instance.id, |current| {
        current.name = name;
        current.mc_version = instance.mc_version.trim().to_string();
        current.loader = instance.loader;
        current.loader_version = if instance.loader == Loader::Fabric {
            non_empty(instance.loader_version)
        } else {
            None
        };
        current.memory_mb = clamp_memory(instance.memory_mb);
        current.jvm_args = instance.jvm_args.trim().to_string();
        current.java_path = non_empty(instance.java_path);
        current.auto_join = non_empty(instance.auto_join);
    })
    .map_err(err)
}

#[tauri::command]
pub fn delete_instance(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    if !is_safe_id(&id) {
        return Err("Invalid instance id.".into());
    }
    if state.running.lock().unwrap().contains_key(&id) {
        return Err("Stop the game before deleting this instance.".into());
    }

    let mut instances = config::load_instances(&state.paths);
    let before = instances.len();
    instances.retain(|i| i.id != id);
    if instances.len() == before {
        return Err("This instance no longer exists.".into());
    }
    config::save_instances(&state.paths, &instances).map_err(err)?;

    for dir in [state.paths.instance_dir(&id), state.paths.natives_dir(&id)] {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(format!(
                    "The instance was removed, but {} could not be deleted: {e}",
                    dir.display()
                ));
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub fn open_instance_folder(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    if !is_safe_id(&id) || !config::load_instances(&state.paths).iter().any(|i| i.id == id) {
        return Err("This instance no longer exists.".into());
    }
    let dir = state.paths.instance_dir(&id);
    std::fs::create_dir_all(&dir).map_err(io_err)?;
    open::that_detached(&dir).map_err(io_err)
}

// ---------- Versions ----------

#[tauri::command]
pub async fn get_versions(state: State<'_, AppState>) -> CmdResult<VersionManifest> {
    versions::get_manifest(state.inner()).await.map_err(err)
}

#[tauri::command]
pub async fn fabric_versions(state: State<'_, AppState>, mc_version: String) -> CmdResult<Vec<LoaderVersion>> {
    fabric::loader_versions(&state.http, &mc_version).await.map_err(err)
}

// ---------- Accounts ----------

#[derive(Serialize)]
pub struct AccountsView {
    active: Option<String>,
    accounts: Vec<AccountView>,
}

#[tauri::command]
pub fn list_accounts(state: State<'_, AppState>) -> AccountsView {
    let store = config::load_accounts(&state.paths);
    AccountsView {
        active: store.active.clone(),
        accounts: store.accounts.iter().map(AccountView::from).collect(),
    }
}

#[tauri::command]
pub async fn login_start(state: State<'_, AppState>) -> CmdResult<DeviceCode> {
    let client_id = config::load_settings(&state.paths).effective_client_id();
    if client_id.is_empty() {
        return Err(auth::NO_CLIENT_ID.into());
    }
    let code = auth::start_device_flow(&state.http, &client_id).await.map_err(err)?;
    *state.login_device_code.lock().unwrap() = Some(code.device_code.clone());
    Ok(code)
}

#[tauri::command]
pub async fn login_wait(
    state: State<'_, AppState>,
    device_code: String,
    interval: u64,
    expires_in: u64,
) -> CmdResult<AccountView> {
    auth::wait_for_login(state.inner(), &device_code, interval, expires_in)
        .await
        .map(|account| AccountView::from(&account))
        .map_err(err)
}

#[tauri::command]
pub fn login_cancel(state: State<'_, AppState>) {
    *state.login_device_code.lock().unwrap() = None;
}

#[tauri::command]
pub async fn set_active_account(state: State<'_, AppState>, uuid: String) -> CmdResult<()> {
    let _lock = state.accounts_lock.lock().await;
    let mut store = config::load_accounts(&state.paths);
    if !store.accounts.iter().any(|a| a.uuid == uuid) {
        return Err("That account is not signed in.".into());
    }
    store.active = Some(uuid);
    config::save_accounts(&state.paths, &store).map_err(err)
}

#[tauri::command]
pub async fn remove_account(state: State<'_, AppState>, uuid: String) -> CmdResult<()> {
    let _lock = state.accounts_lock.lock().await;
    let mut store = config::load_accounts(&state.paths);
    store.accounts.retain(|a| a.uuid != uuid);
    if store.active.as_deref() == Some(uuid.as_str()) {
        store.active = store.accounts.first().map(|a| a.uuid.clone());
    }
    config::save_accounts(&state.paths, &store).map_err(err)
}

// ---------- Game ----------

#[tauri::command]
pub async fn launch_instance(
    app: AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
    server: Option<String>,
) -> CmdResult<()> {
    launch::launch(&app, state.inner(), &instance_id, server)
        .await
        .map_err(err)
}

#[tauri::command]
pub fn kill_instance(state: State<'_, AppState>, instance_id: String) {
    if let Some(kill) = state.running.lock().unwrap().remove(&instance_id) {
        let _ = kill.send(());
    }
}

#[tauri::command]
pub fn running_instances(state: State<'_, AppState>) -> Vec<String> {
    state.running.lock().unwrap().keys().cloned().collect()
}
