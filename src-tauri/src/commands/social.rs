//! Friends commands: the same friends, requests and messages as in Orbit Client.

use serde_json::Value;
use tauri::State;

use crate::commands::auth::active_account;
use crate::error::Result;
use crate::state::AppState;

fn enc(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

async fn call(state: &AppState, path: String, body: Option<String>) -> Result<Value> {
    let account = active_account(state).await?;
    state.social.call(&state.http, &account, &path, body).await
}

/// Friends (online, server), requests, blocks and messages newer than `since`.
///
/// While no game runs this also shows you online "in Orbit Launcher"; while one does, the game
/// reports where you are, so the launcher only reads.
#[tauri::command]
pub async fn social_poll(state: State<'_, AppState>, since: u64) -> Result<Value> {
    let presence = if state.running_games().await.is_empty() { "&server=launcher" } else { "&presence=0" };
    call(&state, format!("/social/poll?since={since}{presence}"), None).await
}

#[tauri::command]
pub async fn social_request(state: State<'_, AppState>, name: String) -> Result<Value> {
    call(&state, format!("/friends/request?name={}", enc(name.trim())), None).await
}

#[tauri::command]
pub async fn social_accept(state: State<'_, AppState>, uuid: String) -> Result<Value> {
    call(&state, format!("/friends/accept?uuid={}", enc(&uuid)), None).await
}

/// Unfriend, decline a request or cancel yours.
#[tauri::command]
pub async fn social_remove(state: State<'_, AppState>, uuid: String) -> Result<Value> {
    call(&state, format!("/friends/remove?uuid={}", enc(&uuid)), None).await
}

#[tauri::command]
pub async fn social_block(state: State<'_, AppState>, uuid: String, block: bool) -> Result<Value> {
    let route = if block { "block" } else { "unblock" };
    call(&state, format!("/{route}?uuid={}", enc(&uuid)), None).await
}

#[tauri::command]
pub async fn social_message(state: State<'_, AppState>, to: String, text: String) -> Result<Value> {
    call(&state, format!("/messages/send?to={}", enc(&to)), Some(text)).await
}
