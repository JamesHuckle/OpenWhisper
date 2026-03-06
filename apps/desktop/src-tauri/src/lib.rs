mod worker_client;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};
use tokio::sync::Mutex;
use worker_client::WorkerClient;

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct AppSettings {
    openai_api_key: String,
}

struct AppState {
    worker: Mutex<Option<WorkerClient>>,
    settings: std::sync::Mutex<AppSettings>,
}

#[derive(Debug, Serialize)]
struct SessionStartResponse {
    session_id: String,
}

async fn with_worker_request(
    app_handle: &AppHandle,
    state: &State<'_, Arc<AppState>>,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let mut guard = state.worker.lock().await;
    if guard.is_none() {
        let openai_api_key = {
            let settings = state
                .settings
                .lock()
                .map_err(|_| "Failed to lock settings".to_string())?;
            if settings.openai_api_key.trim().is_empty() {
                None
            } else {
                Some(settings.openai_api_key.clone())
            }
        };
        let worker = WorkerClient::spawn(app_handle, openai_api_key)
            .await
            .map_err(|e| e.to_string())?;
        *guard = Some(worker);
    }

    let worker = guard
        .as_mut()
        .ok_or_else(|| "Worker initialization failed".to_string())?;

    worker
        .request(method, params)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn app_get_settings(state: State<'_, Arc<AppState>>) -> Result<AppSettings, String> {
    let settings = state
        .settings
        .lock()
        .map_err(|_| "Failed to lock settings".to_string())?
        .clone();
    Ok(settings)
}

#[tauri::command]
async fn app_save_settings(
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
    openai_api_key: String,
) -> Result<AppSettings, String> {
    let updated = AppSettings {
        openai_api_key: openai_api_key.trim().to_string(),
    };
    save_settings(&app_handle, &updated).map_err(|e| e.to_string())?;

    {
        let mut settings = state
            .settings
            .lock()
            .map_err(|_| "Failed to lock settings".to_string())?;
        *settings = updated.clone();
    }

    // Force a fresh worker spawn so new settings are applied.
    let mut worker = state.worker.lock().await;
    *worker = None;

    Ok(updated)
}

#[tauri::command]
async fn worker_ping(
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<serde_json::Value, String> {
    with_worker_request(&app_handle, &state, "ping", json!({})).await
}

#[tauri::command]
async fn worker_list_models(
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<serde_json::Value, String> {
    with_worker_request(&app_handle, &state, "list_models", json!({})).await
}

#[tauri::command]
async fn worker_start_session(
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
    profile_id: String,
) -> Result<SessionStartResponse, String> {
    let response = with_worker_request(
        &app_handle,
        &state,
        "start_session",
        json!({ "profile_id": profile_id }),
    )
    .await?;

    let session_id = response
        .get("session_id")
        .and_then(|x| x.as_str())
        .ok_or_else(|| "Missing session_id from worker".to_string())?
        .to_string();

    Ok(SessionStartResponse { session_id })
}

#[tauri::command]
async fn worker_stop_session(
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<serde_json::Value, String> {
    with_worker_request(
        &app_handle,
        &state,
        "stop_session",
        json!({ "session_id": session_id }),
    )
    .await
}

#[tauri::command]
async fn worker_poll_session_events(
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<serde_json::Value, String> {
    with_worker_request(
        &app_handle,
        &state,
        "poll_session_events",
        json!({ "session_id": session_id }),
    )
    .await
}

#[tauri::command]
async fn worker_append_audio_chunk(
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    chunk_base64: String,
) -> Result<serde_json::Value, String> {
    with_worker_request(
        &app_handle,
        &state,
        "append_audio_chunk",
        json!({
            "session_id": session_id,
            "chunk_base64": chunk_base64
        }),
    )
    .await
}

#[tauri::command]
async fn worker_finalize_session_audio(
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    mime_type: String,
) -> Result<serde_json::Value, String> {
    with_worker_request(
        &app_handle,
        &state,
        "finalize_session_audio",
        json!({
            "session_id": session_id,
            "mime_type": mime_type
        }),
    )
    .await
}

fn settings_path(app_handle: &AppHandle) -> Result<PathBuf> {
    let mut dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| anyhow!("Failed to resolve app config directory: {e}"))?;
    dir.push("settings.json");
    Ok(dir)
}

fn load_settings(app_handle: &AppHandle) -> Result<AppSettings> {
    let path = settings_path(app_handle)?;
    if !path.exists() {
        return Ok(AppSettings::default());
    }
    let raw = fs::read_to_string(&path)?;
    let settings: AppSettings = serde_json::from_str(&raw)?;
    Ok(settings)
}

fn save_settings(app_handle: &AppHandle, settings: &AppSettings) -> Result<()> {
    let path = settings_path(app_handle)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = serde_json::to_string_pretty(settings)?;
    fs::write(path, payload)?;
    Ok(())
}

pub fn run() {
    let app_state = Arc::new(AppState {
        worker: Mutex::new(None),
        settings: std::sync::Mutex::new(AppSettings::default()),
    });

    tauri::Builder::default()
        .manage(app_state)
        .setup(|app| {
            let loaded = load_settings(app.handle()).unwrap_or_default();
            let state = app.state::<Arc<AppState>>();
            let mut settings = state
                .settings
                .lock()
                .map_err(|_| anyhow!("Failed to lock settings during setup"))?;
            *settings = loaded;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_get_settings,
            app_save_settings,
            worker_ping,
            worker_list_models,
            worker_start_session,
            worker_stop_session,
            worker_poll_session_events,
            worker_append_audio_chunk,
            worker_finalize_session_audio
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

