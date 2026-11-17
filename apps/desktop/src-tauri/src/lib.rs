mod api_key_store;
mod debug_log;
mod flac_upload;
#[cfg(target_os = "windows")]
mod foreground_app;
mod json_file;
// Parts of the insertion rules are used only by the Windows writer.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod paste_target;
mod models;
// The pill is placed with Win32 calls, so most of this is used only on Windows.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod overlay_geometry;
#[cfg(target_os = "windows")]
mod overlay_positioner;
mod recording_store;
mod recordings;
mod settings;
// Writes dictated text into the field focused when recording began.
#[cfg(target_os = "windows")]
mod text_inserter;
mod worker_client;
mod worker_connection;

use anyhow::{anyhow, Result};
use base64::Engine;
use debug_log::debug_log_line;
use models::{is_supported_transcription_model, migrate_transcription_model};
use overlay_geometry::{
    overlay_placement, MonitorTransitionDebouncer, OverlayAnchor, OverlayLayoutSpec,
    OverlayPlacement, OverlayRevealIntent, PlacementVisibility,
};
use recording_store::PcmWavWriter;
use recordings::{
    close_active_recording, finish_recording, import_recording, list_recording_metadata,
    lock_recordings, now_ms, read_recording_metadata, recording_parts_path,
    write_recording_metadata, ActiveRecording, RecordingMetadata, RecordingStartResponse,
};
use serde::Serialize;
use serde_json::json;
use settings::{frontend_settings_from, start_session_params, AppSettings, FrontendSettings};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, LogicalSize, Manager, Size, State, Window, WindowEvent,
};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::Mutex;
use uuid::Uuid;
use worker_client::WorkerClient;

#[cfg(target_os = "windows")]
fn primary_overlay_anchor() -> Option<OverlayAnchor> {
    overlay_positioner::primary_monitor_anchor().map(|(monitor_id, center_x, bottom_y, dpi)| {
        OverlayAnchor {
            monitor_id,
            center_x,
            bottom_y,
            dpi,
        }
    })
}

#[cfg(target_os = "windows")]
fn cursor_overlay_anchor() -> Option<OverlayAnchor> {
    overlay_positioner::cursor_monitor_anchor().map(|(monitor_id, center_x, bottom_y, dpi)| {
        OverlayAnchor {
            monitor_id,
            center_x,
            bottom_y,
            dpi,
        }
    })
}

/// How often the pill checks which monitor the cursor is on.
#[cfg(target_os = "windows")]
const MONITOR_FOLLOW_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

struct AppState {
    worker: Mutex<Option<Arc<WorkerClient>>>,
    settings: std::sync::Mutex<AppSettings>,
    overlay_anchor: std::sync::Mutex<Option<OverlayAnchor>>,
    overlay_layout: std::sync::Mutex<Option<OverlayLayoutSpec>>,
    overlay_layout_locked: AtomicBool,
    // Microphone recordings in progress, by recording id. Each keeps its
    // file open until the recording is transcribed or finished.
    recordings: std::sync::Mutex<HashMap<String, ActiveRecording>>,
}

struct OverlayLayoutGuard<'a>(&'a AtomicBool);

impl<'a> OverlayLayoutGuard<'a> {
    fn try_acquire(flag: &'a AtomicBool) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self(flag))
    }
}

impl Drop for OverlayLayoutGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn resolve_overlay_anchor(state: &Arc<AppState>) -> Result<Option<OverlayAnchor>, String> {
    let mut guard = state
        .overlay_anchor
        .lock()
        .map_err(|_| "Failed to lock overlay anchor".to_string())?;

    #[cfg(target_os = "windows")]
    if guard.is_none() {
        if let Some(anchor) = cursor_overlay_anchor().or_else(primary_overlay_anchor) {
            *guard = Some(anchor);
        }
    }

    Ok(*guard)
}

#[cfg(target_os = "windows")]
fn commit_native_placement(
    hwnd: isize,
    placement: OverlayPlacement,
    visibility: PlacementVisibility,
) -> Result<(), String> {
    overlay_positioner::set_window_placement(hwnd, placement.position, placement.size, visibility)
}

#[cfg(target_os = "windows")]
fn native_placement_matches(hwnd: isize, expected: OverlayPlacement) -> Result<bool, String> {
    let observed = overlay_positioner::window_placement(hwnd)
        .ok_or_else(|| "GetWindowRect failed while verifying overlay placement".to_string())?;
    Ok(
        observed.0 == expected.position
            && observed.1 == expected.size
            && observed.2 == expected.dpi,
    )
}

#[cfg(target_os = "windows")]
fn sync_overlay_window_to_anchor(app: &AppHandle, anchor: OverlayAnchor) -> Result<bool, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let Some(_layout_guard) = OverlayLayoutGuard::try_acquire(&state.overlay_layout_locked) else {
        return Ok(false);
    };

    {
        let current = state
            .overlay_anchor
            .lock()
            .map_err(|_| "Failed to lock overlay anchor".to_string())?;
        if *current == Some(anchor) {
            return Ok(true);
        }
    }

    let layout = *state
        .overlay_layout
        .lock()
        .map_err(|_| "Failed to lock overlay layout".to_string())?;
    let Some(layout) = layout else {
        return Ok(false);
    };
    let Some(window) = app.get_webview_window("main") else {
        return Ok(false);
    };
    let hwnd = window.hwnd().map_err(|e| e.to_string())?.0 as isize;
    let placement = overlay_placement(anchor, layout);
    let was_visible = window.is_visible().map_err(|e| e.to_string())?;

    // A monitor move can dispatch WM_DPICHANGED after SetWindowPos returns.
    // Keep every intermediate rectangle hidden and non-redrawing, then expose
    // exactly one already-settled rectangle to the compositor.
    commit_native_placement(hwnd, placement, PlacementVisibility::HideWithoutRedraw)?;
    let mut stable_samples = 0u8;
    let mut correction_count = 0u8;
    for _ in 0..12 {
        std::thread::sleep(std::time::Duration::from_millis(8));
        if native_placement_matches(hwnd, placement)? {
            stable_samples = stable_samples.saturating_add(1);
            if stable_samples >= 2 {
                break;
            }
        } else {
            stable_samples = 0;
            correction_count = correction_count.saturating_add(1);
            commit_native_placement(hwnd, placement, PlacementVisibility::HideWithoutRedraw)?;
        }
    }
    if !native_placement_matches(hwnd, placement)? {
        return Err(
            "Windows did not settle the overlay at its committed DPI placement".to_string(),
        );
    }
    if was_visible {
        commit_native_placement(hwnd, placement, PlacementVisibility::Show)?;
        overlay_positioner::reassert_topmost(hwnd);
    }
    debug_log_line(
        "window",
        &format!(
            "monitor commit id={} anchor=({}, {}) dpi={} position=({}, {}) size={}x{} hidden_corrections={}",
            anchor.monitor_id,
            anchor.center_x,
            anchor.bottom_y,
            anchor.dpi,
            placement.position.x,
            placement.position.y,
            placement.size.width,
            placement.size.height,
            correction_count
        ),
    );

    let mut current = state
        .overlay_anchor
        .lock()
        .map_err(|_| "Failed to lock overlay anchor".to_string())?;
    *current = Some(anchor);
    Ok(true)
}

#[cfg(target_os = "windows")]
fn position_main_window_before_reveal(app: &AppHandle) -> Result<bool, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let Some(window) = app.get_webview_window("main") else {
        return Ok(false);
    };
    let Some(anchor) = resolve_overlay_anchor(&state)? else {
        return Ok(false);
    };
    let layout = *state
        .overlay_layout
        .lock()
        .map_err(|_| "Failed to lock overlay layout".to_string())?;
    let Some(layout) = layout else {
        return Ok(false);
    };
    commit_native_placement(
        window.hwnd().map_err(|e| e.to_string())?.0 as isize,
        overlay_placement(anchor, layout),
        PlacementVisibility::Keep,
    )?;
    Ok(true)
}

#[derive(Debug, Serialize)]
struct SessionStartResponse {
    session_id: String,
}

fn recordings_root(app_handle: &AppHandle) -> Result<PathBuf> {
    let mut root = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| anyhow!("Failed to resolve app data directory: {e}"))?;
    root.push("recordings");
    Ok(root)
}

// The recording commands create, write and list files, which antivirus
// scans can slow on Windows. `async` runs them off the main thread, so the
// pill never freezes; the page sends a recording's chunks one at a time, so
// they still arrive in order.
#[tauri::command(async)]
fn recording_begin(
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
    source: String,
    name: String,
) -> Result<RecordingStartResponse, String> {
    let root = recordings_root(&app_handle).map_err(|e| e.to_string())?;
    // One microphone recording runs at a time. Close any the UI left open
    // (for example after a reload) so their files are complete.
    let left_open: Vec<String> = lock_recordings(&state.recordings)?.keys().cloned().collect();
    for recording_id in left_open {
        close_active_recording(&state.recordings, &root, &recording_id)?;
    }
    let id = Uuid::new_v4().to_string();
    let dir = root.join(&id);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let writer = PcmWavWriter::create(
        &dir.join("audio.wav"),
        recording_store::MICROPHONE_SAMPLE_RATE,
        Instant::now(),
    )
    .map_err(|e| e.to_string())?;
    let metadata = RecordingMetadata {
        id: id.clone(),
        created_at_ms: now_ms(),
        name,
        source,
        mime_type: "audio/wav".to_string(),
        file_name: "audio.wav".to_string(),
        status: "recording".to_string(),
        bytes: writer.file_bytes(),
        transcript: String::new(),
        error: String::new(),
    };
    write_recording_metadata(&root, &metadata).map_err(|e| e.to_string())?;
    lock_recordings(&state.recordings)?.insert(id.clone(), ActiveRecording { writer, metadata });
    // Finishing or importing a recording prunes old ones, not starting it.
    Ok(RecordingStartResponse { id })
}

#[tauri::command(async)]
fn recording_append_chunk(
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
    recording_id: String,
    chunk_base64: String,
) -> Result<(), String> {
    let chunk = base64::engine::general_purpose::STANDARD
        .decode(chunk_base64)
        .map_err(|e| format!("Invalid audio chunk: {e}"))?;
    let mut recordings = lock_recordings(&state.recordings)?;
    let active = recordings
        .get_mut(&recording_id)
        .ok_or_else(|| "This recording is no longer being saved".to_string())?;
    let synced = active
        .writer
        .append(&chunk, Instant::now())
        .map_err(|e| e.to_string())?;
    if synced {
        // The saved size follows the header, about once a second.
        active.metadata.bytes = active.writer.file_bytes();
        let root = recordings_root(&app_handle).map_err(|e| e.to_string())?;
        write_recording_metadata(&root, &active.metadata).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Lets the user choose an audio file and copies it into the saved
/// recordings, straight from disk. Returns None when the user cancels.
#[tauri::command]
async fn recording_import(app_handle: AppHandle) -> Result<Option<RecordingStartResponse>, String> {
    let dialog_app = app_handle.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = dialog_app
            .dialog()
            .file()
            .set_title("Choose an audio file to transcribe")
            .add_filter("Audio", recording_store::IMPORT_EXTENSIONS);
        if let Some(window) = dialog_app.get_webview_window("main") {
            dialog = dialog.set_parent(&window);
        }
        dialog.blocking_pick_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let source = picked.into_path().map_err(|e| e.to_string())?;
    let root = recordings_root(&app_handle).map_err(|e| e.to_string())?;
    // A large file copies on a blocking thread, not an async worker.
    tauri::async_runtime::spawn_blocking(move || import_recording(&root, &source))
        .await
        .map_err(|e| e.to_string())?
        .map(Some)
}

/// Transcribes a saved recording in a worker session. The worker reads the
/// file from disk, so its audio never passes through the UI.
#[tauri::command]
async fn recording_transcribe(
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
    recording_id: String,
    session_id: String,
) -> Result<serde_json::Value, String> {
    let root = recordings_root(&app_handle).map_err(|e| e.to_string())?;
    close_active_recording(&state.recordings, &root, &recording_id)?;
    let metadata = read_recording_metadata(&root, &recording_id).map_err(|e| e.to_string())?;
    let audio_path = root.join(&recording_id).join(&metadata.file_name);
    let mut params = json!({
        "session_id": session_id,
        "mime_type": metadata.mime_type,
        "audio_path": audio_path.to_string_lossy(),
        // A long recording's finished parts are kept here, so a retry after
        // a failure sends only the rest. Removed once it completes.
        "parts_path": recording_parts_path(&root, &recording_id).to_string_lossy(),
    });
    let mut upload_path = None;
    if metadata.source == "microphone" {
        // A recording the app did not finish (it stopped while recording)
        // has a header up to a second behind its audio.
        recording_store::repair_wav_header(&audio_path, recording_store::MICROPHONE_SAMPLE_RATE)
            .map_err(|e| format!("Could not read saved recording: {e}"))?;
        upload_path = flac_upload_copy(audio_path.clone()).await;
        if let Some(path) = &upload_path {
            params["upload_path"] = json!(path.to_string_lossy());
            params["upload_mime_type"] = json!(flac_upload::MIME_TYPE);
        }
    }
    let result = with_worker_request(&app_handle, &state, "finalize_session_audio", params).await;
    if let Some(path) = upload_path {
        let _ = fs::remove_file(path);
    }
    result
}

/// Writes the smaller, lossless FLAC copy of a microphone recording that the
/// worker uploads in place of its WAV. None when there is no copy: the worker
/// then sends the WAV, as it does for a copy the service refuses.
async fn flac_upload_copy(wav_path: PathBuf) -> Option<PathBuf> {
    let flac_path = wav_path.with_file_name(flac_upload::FILE_NAME);
    let target = flac_path.clone();
    let written = tauri::async_runtime::spawn_blocking(move || {
        flac_upload::write_upload_copy(&wav_path, &target, flac_upload::max_upload_bytes())
    })
    .await;
    match written {
        Ok(Ok(true)) => Some(flac_path),
        Ok(Ok(false)) => None,
        Ok(Err(error)) => {
            debug_log_line("recording", &format!("FLAC upload copy failed: {error}"));
            None
        }
        Err(error) => {
            debug_log_line("recording", &format!("FLAC upload copy stopped: {error}"));
            None
        }
    }
}

#[tauri::command(async)]
fn recording_finish(
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
    recording_id: String,
    status: String,
    transcript: String,
    error: String,
) -> Result<(), String> {
    let root = recordings_root(&app_handle).map_err(|e| e.to_string())?;
    close_active_recording(&state.recordings, &root, &recording_id)?;
    finish_recording(&root, &recording_id, status, transcript, error).map_err(|e| e.to_string())
}

#[tauri::command(async)]
fn recording_list(app_handle: AppHandle) -> Result<Vec<RecordingMetadata>, String> {
    let root = recordings_root(&app_handle).map_err(|e| e.to_string())?;
    list_recording_metadata(&root).map_err(|e| e.to_string())
}

#[tauri::command]
fn recording_open(app_handle: AppHandle, recording_id: String) -> Result<(), String> {
    let root = recordings_root(&app_handle).map_err(|e| e.to_string())?;
    let metadata = read_recording_metadata(&root, &recording_id).map_err(|e| e.to_string())?;
    let audio_path = root.join(recording_id).join(metadata.file_name);
    app_handle
        .opener()
        .open_path(audio_path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn recordings_open_folder(app_handle: AppHandle) -> Result<(), String> {
    let root = recordings_root(&app_handle).map_err(|e| e.to_string())?;
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    app_handle
        .opener()
        .open_path(root.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

async fn with_worker_request(
    app_handle: &AppHandle,
    state: &State<'_, Arc<AppState>>,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    // Hold the lock only to find or start the worker, never while a request
    // runs, so a cancel is not stuck behind a transcription. The worker is
    // started again only after it stops; settings changes travel with each
    // dictation instead.
    let worker = {
        let mut guard = state.worker.lock().await;
        match guard.as_ref() {
            Some(worker) if worker.is_alive() => worker.clone(),
            _ => {
                let worker = Arc::new(
                    WorkerClient::spawn(app_handle)
                        .await
                        .map_err(|e| e.to_string())?,
                );
                *guard = Some(worker.clone());
                worker
            }
        }
    };

    worker
        .request(method, params)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn app_get_settings(state: State<'_, Arc<AppState>>) -> Result<FrontendSettings, String> {
    let settings = state
        .settings
        .lock()
        .map_err(|_| "Failed to lock settings".to_string())?
        .clone();
    Ok(frontend_settings_from(&settings))
}

#[tauri::command]
async fn app_get_openai_api_key(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    let settings = state
        .settings
        .lock()
        .map_err(|_| "Failed to lock settings".to_string())?
        .clone();
    Ok(settings.openai_api_key)
}

#[tauri::command]
async fn app_save_settings(
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
    openai_api_key: Option<String>,
    transcription_model: Option<String>,
    transcription_prompt: Option<String>,
    refine_enabled: Option<bool>,
    refine_prompt: Option<String>,
    show_idle_pill: Option<bool>,
    hold_to_talk: Option<bool>,
) -> Result<FrontendSettings, String> {
    // One save at a time, from reading the settings to storing them, so two
    // saves never undo each other's change or API key.
    static SAVING_SETTINGS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _saving = SAVING_SETTINGS.lock().unwrap_or_else(|e| e.into_inner());
    let mut previous_key = None;
    let updated = {
        let mut current = state
            .settings
            .lock()
            .map_err(|_| "Failed to lock settings".to_string())?
            .clone();
        if let Some(model) = transcription_model {
            if !is_supported_transcription_model(&model) {
                return Err(format!("Unsupported transcription model: {model}"));
            }
            current.transcription_model = model;
        }
        if let Some(prompt) = transcription_prompt {
            current.transcription_prompt = prompt;
        }
        if let Some(enabled) = refine_enabled {
            current.refine_enabled = enabled;
        }
        if let Some(prompt) = refine_prompt {
            current.refine_prompt = prompt;
        }
        if let Some(show) = show_idle_pill {
            current.show_idle_pill = show;
        }
        if let Some(enabled) = hold_to_talk {
            current.hold_to_talk = enabled;
        }
        // Last, after every check above, so a rejected save leaves the stored
        // key untouched.
        if let Some(key) = openai_api_key {
            let key = api_key_store::replace(&CredentialStore, &key)
                .map_err(|e| format!("Could not save the API key securely: {e}"))?;
            previous_key = Some(std::mem::replace(&mut current.openai_api_key, key));
            current.plain_openai_api_key.clear();
        }
        current
    };
    if let Err(err) = save_settings(&app_handle, &updated) {
        // The settings stay as they were, so the stored key must too.
        if let Some(previous) = previous_key {
            if let Err(restore) = api_key_store::replace(&CredentialStore, &previous) {
                return Err(format!(
                    "{err}; the previous API key could not be put back: {restore}"
                ));
            }
        }
        return Err(err.to_string());
    }

    {
        let mut settings = state
            .settings
            .lock()
            .map_err(|_| "Failed to lock settings".to_string())?;
        *settings = updated.clone();
    }

    #[cfg(target_os = "windows")]
    text_inserter::set_hold_to_talk_enabled(updated.hold_to_talk);

    // The worker keeps running: the next dictation sends the new settings
    // with its start_session request, so it does not wait for a cold start.
    Ok(frontend_settings_from(&updated))
}

#[tauri::command]
async fn worker_ping(
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<serde_json::Value, String> {
    with_worker_request(&app_handle, &state, "ping", json!({})).await
}

#[tauri::command]
async fn worker_start_session(
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
    profile_id: String,
) -> Result<SessionStartResponse, String> {
    // Every setting the worker uses travels with the session, so a saved
    // change applies to the next dictation without restarting the worker.
    let params = {
        let s = state
            .settings
            .lock()
            .map_err(|_| "Failed to lock settings".to_string())?;
        start_session_params(&s, &profile_id)
    };

    let response = with_worker_request(&app_handle, &state, "start_session", params).await?;

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

#[tauri::command]
fn debug_log(message: String) -> Result<(), String> {
    debug_log_line("frontend", &message);
    Ok(())
}

#[tauri::command]
fn get_foreground_app() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        text_inserter::foreground_app_name()
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

#[tauri::command]
fn open_api_keys_page(app_handle: AppHandle) -> Result<(), String> {
    app_handle
        .opener()
        .open_url("https://platform.openai.com/api-keys", None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn copy_to_clipboard(text: String) -> Result<(), String> {
    arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.set_text(text))
        .map_err(|e| e.to_string())
}

/// Lock the field the user is in for the whole recording.
#[tauri::command]
fn capture_paste_target() {
    #[cfg(target_os = "windows")]
    text_inserter::capture_target();
}

/// How a write into the recording's target field ended. `outcome` is one of
/// `typed`, `stopped` (live typing ended for this recording), `replaced`,
/// `pasted`, `kept` (the live draft already matched) or `copied` (left on the
/// clipboard instead); `reason` says why text was stopped or copied.
#[derive(Debug, Serialize)]
struct TargetWrite {
    outcome: &'static str,
    reason: Option<&'static str>,
}

#[cfg(target_os = "windows")]
impl From<text_inserter::WriteOutcome> for TargetWrite {
    fn from(outcome: text_inserter::WriteOutcome) -> Self {
        use text_inserter::WriteOutcome::*;
        let (outcome, reason) = match outcome {
            Typed => ("typed", None),
            Replaced => ("replaced", None),
            Pasted => ("pasted", None),
            Kept => ("kept", None),
            Stopped(blocked) => ("stopped", Some(blocked.as_str())),
            Copied(blocked) => ("copied", Some(blocked.as_str())),
        };
        TargetWrite { outcome, reason }
    }
}

/// Type one live piece of the transcript into the locked field.
#[tauri::command]
async fn type_to_target(text: String) -> Result<TargetWrite, String> {
    #[cfg(target_os = "windows")]
    {
        tauri::async_runtime::spawn_blocking(move || text_inserter::type_live(&text).into())
            .await
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = text;
        Ok(TargetWrite {
            outcome: "stopped",
            reason: Some("unsupported"),
        })
    }
}

/// Write the final transcript into the locked field, replacing the live
/// draft only when it is safe to delete.
#[tauri::command]
async fn insert_final_text(text: String) -> Result<TargetWrite, String> {
    #[cfg(target_os = "windows")]
    {
        tauri::async_runtime::spawn_blocking(move || text_inserter::insert_final(&text))
            .await
            .map_err(|e| e.to_string())?
            .map(TargetWrite::from)
    }
    #[cfg(not(target_os = "windows"))]
    {
        copy_to_clipboard(text)?;
        Ok(TargetWrite {
            outcome: "copied",
            reason: Some("unsupported"),
        })
    }
}

#[tauri::command]
async fn overlay_apply_layout(
    window: Window,
    state: State<'_, Arc<AppState>>,
    width: f64,
    height: f64,
    anchor_offset_y: f64,
    visible: bool,
) -> Result<(), String> {
    let app_state = state.inner().clone();

    #[cfg(target_os = "windows")]
    {
        let _layout_guard = loop {
            if let Some(guard) = OverlayLayoutGuard::try_acquire(&app_state.overlay_layout_locked) {
                break guard;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        };
        let layout = OverlayLayoutSpec {
            width,
            height,
            anchor_offset_y,
        };
        *app_state
            .overlay_layout
            .lock()
            .map_err(|_| "Failed to lock overlay layout".to_string())? = Some(layout);
        let anchor = resolve_overlay_anchor(&app_state)?;

        if let Some(anchor) = anchor {
            let placement = overlay_placement(anchor, layout);
            let was_visible = window.is_visible().map_err(|e| e.to_string())?;
            let hwnd = window.hwnd().map_err(|e| e.to_string())?.0 as isize;
            debug_log_line(
                "layout",
                &format!(
                    "commit logical={}x{} anchor=({}, {}) dpi={} physical={}x{} position=({}, {}) visible={}",
                    width,
                    height,
                    anchor.center_x,
                    anchor.bottom_y,
                    anchor.dpi,
                    placement.size.width,
                    placement.size.height,
                    placement.position.x,
                    placement.position.y,
                    was_visible
                ),
            );
            let visibility = match (was_visible, visible) {
                (true, true) => PlacementVisibility::Keep,
                (false, true) => PlacementVisibility::HideWithoutRedraw,
                (_, false) => PlacementVisibility::HideWithoutRedraw,
            };
            commit_native_placement(hwnd, placement, visibility)?;

            if visible && !was_visible {
                let mut stable_samples = 0u8;
                for _ in 0..12 {
                    tokio::time::sleep(std::time::Duration::from_millis(8)).await;
                    if native_placement_matches(hwnd, placement)? {
                        stable_samples = stable_samples.saturating_add(1);
                        if stable_samples >= 2 {
                            break;
                        }
                    } else {
                        stable_samples = 0;
                        commit_native_placement(
                            hwnd,
                            placement,
                            PlacementVisibility::HideWithoutRedraw,
                        )?;
                    }
                }
                if !native_placement_matches(hwnd, placement)? {
                    return Err("Initial overlay placement did not settle".to_string());
                }
                commit_native_placement(hwnd, placement, PlacementVisibility::Show)?;
            }
            // The recording pill can appear while the window is already up (an
            // idle→recording transition is a Keep, not a Show). Re-raise it to
            // the top of the topmost band on every visible layout commit so the
            // active listening pill is never occluded by other topmost windows.
            if visible {
                overlay_positioner::reassert_topmost(hwnd);
            }
            return Ok(());
        }
    }

    window
        .set_size(Size::Logical(LogicalSize::new(width, height)))
        .map_err(|e| e.to_string())?;
    if visible {
        window.show().map_err(|e| e.to_string())?;
    } else {
        window.hide().map_err(|e| e.to_string())?;
    }

    Ok(())
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
    let Some(mut settings) = json_file::read_json_file::<AppSettings>(&path)? else {
        return Ok(AppSettings::default());
    };
    settings.transcription_model = migrate_transcription_model(&settings.transcription_model);
    Ok(settings)
}

/// Loads the settings and the API key, moving a plain-text key from the
/// settings file into the OS credential store.
fn load_settings_and_api_key(app_handle: &AppHandle) -> AppSettings {
    let mut settings = load_settings(app_handle).unwrap_or_else(|e| {
        debug_log_line("settings", &format!("load failed, using defaults: {e}"));
        AppSettings::default()
    });
    let loaded = api_key_store::load(&CredentialStore, &settings.plain_openai_api_key);
    settings.openai_api_key = loaded.key;
    if !loaded.keep_plain_copy {
        settings.plain_openai_api_key.clear();
    }
    match loaded.outcome {
        api_key_store::LoadOutcome::Ready => {}
        api_key_store::LoadOutcome::Migrated => {
            // The key is safe in the credential store, so the plain copy can
            // go. If this write fails, the move is repeated on the next start.
            match save_settings(app_handle, &settings) {
                Ok(()) => debug_log_line("settings", "moved API key to credential store"),
                Err(e) => debug_log_line(
                    "settings",
                    &format!("API key stored securely, plain copy not yet removed: {e}"),
                ),
            }
        }
        api_key_store::LoadOutcome::MigrationFailed(e) => debug_log_line(
            "settings",
            &format!("could not move API key to credential store, keeping it: {e}"),
        ),
        api_key_store::LoadOutcome::ReadFailed(e) => {
            debug_log_line("settings", &format!("could not read API key: {e}"))
        }
    }
    settings
}

const API_KEY_CREDENTIAL_SERVICE: &str = "com.openwhisper.app";
const API_KEY_CREDENTIAL_USER: &str = "openai-api-key";

/// The OS credential store: Windows Credential Manager, the macOS Keychain or
/// the Linux Secret Service.
struct CredentialStore;

impl CredentialStore {
    fn entry() -> Result<keyring::Entry, String> {
        keyring::Entry::new(API_KEY_CREDENTIAL_SERVICE, API_KEY_CREDENTIAL_USER)
            .map_err(|e| e.to_string())
    }
}

impl api_key_store::SecretStore for CredentialStore {
    fn read(&self) -> Result<Option<String>, String> {
        match Self::entry()?.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn write(&self, key: &str) -> Result<(), String> {
        Self::entry()?.set_password(key).map_err(|e| e.to_string())
    }

    fn delete(&self) -> Result<(), String> {
        match Self::entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

fn save_settings(app_handle: &AppHandle, settings: &AppSettings) -> Result<()> {
    let path = settings_path(app_handle)?;
    json_file::write_json_atomic(&path, settings)?;
    // The backup is the previous file. Once the plain-text API key has left
    // the settings it must not stay behind in the backup either.
    if settings.plain_openai_api_key.is_empty() && backup_holds_plain_api_key(&path) {
        json_file::refresh_backup(&path)?;
    }
    Ok(())
}

fn backup_holds_plain_api_key(settings_path: &std::path::Path) -> bool {
    fs::read(json_file::backup_path(settings_path))
        .ok()
        .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
        .is_some_and(|backup| backup.get("openaiApiKey").is_some())
}

fn make_tray_icon() -> tauri::image::Image<'static> {
    let bytes = include_bytes!("../icons/32x32.png");
    tauri::image::Image::from_bytes(bytes)
        .expect("tray icon 32x32.png")
        .to_owned()
}

fn reveal_window(app: &AppHandle, intent: OverlayRevealIntent) {
    if matches!(intent, OverlayRevealIntent::Passive) {
        let show_idle_pill = app
            .state::<Arc<AppState>>()
            .settings
            .lock()
            .map(|settings| settings.show_idle_pill)
            .unwrap_or(true);
        if !show_idle_pill {
            debug_log_line("window", "passive reveal skipped while idle pill is hidden");
            return;
        }
    }
    if let Some(window) = app.get_webview_window("main") {
        #[cfg(target_os = "windows")]
        match position_main_window_before_reveal(app) {
            Ok(true) => {}
            Ok(false) => {
                debug_log_line("window", "reveal deferred until layout is committed");
                return;
            }
            Err(err) => {
                debug_log_line(
                    "window",
                    &format!("position_main_window_before_reveal failed: {err}"),
                );
                return;
            }
        }

        debug_log_line("window", "reveal_window");
        let _ = window.show();
        let _ = window.unminimize();
        #[cfg(target_os = "windows")]
        if let Ok(hwnd) = window.hwnd() {
            overlay_positioner::reassert_topmost(hwnd.0 as isize);
        }
        if intent.should_focus() {
            let _ = window.set_focus();
        }
        let _ = window.emit("overlay-revealed", intent.should_focus());
    }
}

fn reveal_main_window(app: &AppHandle) {
    reveal_window(app, OverlayRevealIntent::Interactive);
}

fn reveal_overlay_window(app: &AppHandle) {
    reveal_window(app, OverlayRevealIntent::Passive);
}

fn setup_tray(app: &tauri::App) -> Result<()> {
    let show_i = MenuItem::with_id(app, "show", "Show Pill", true, None::<&str>)
        .map_err(|e| anyhow!("{e}"))?;
    let record_i = MenuItem::with_id(app, "record", "Toggle Recording", true, None::<&str>)
        .map_err(|e| anyhow!("{e}"))?;
    let sep = PredefinedMenuItem::separator(app).map_err(|e| anyhow!("{e}"))?;
    let quit_i = MenuItem::with_id(app, "quit", "Quit OpenWhisper", true, None::<&str>)
        .map_err(|e| anyhow!("{e}"))?;

    let menu =
        Menu::with_items(app, &[&show_i, &record_i, &sep, &quit_i]).map_err(|e| anyhow!("{e}"))?;

    TrayIconBuilder::new()
        .icon(make_tray_icon())
        .tooltip("OpenWhisper – Ctrl+Space to talk")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => reveal_main_window(app),
            "record" => {
                #[cfg(target_os = "windows")]
                text_inserter::track_foreground();
                reveal_main_window(app);
                let _ = app.emit("toggle-recording", ());
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                reveal_main_window(&app);
            }
        })
        .build(app)
        .map_err(|e| anyhow!("{e}"))?;

    Ok(())
}

pub fn run() {
    let app_state = Arc::new(AppState {
        worker: Mutex::new(None),
        settings: std::sync::Mutex::new(AppSettings::default()),
        overlay_anchor: std::sync::Mutex::new(None),
        overlay_layout: std::sync::Mutex::new(None),
        overlay_layout_locked: AtomicBool::new(false),
        recordings: std::sync::Mutex::new(HashMap::new()),
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(app_state)
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .args(["--autostart"])
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }

            match event {
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    reveal_overlay_window(&window.app_handle());
                }
                WindowEvent::Focused(false) => {
                    if std::env::var_os("OPENWHISPER_UI_TEST").is_none() {
                        let _ = window.emit("overlay-lost-focus", ());
                    }
                }
                _ => {}
            }
        })
        .setup(|app| {
            let loaded = load_settings_and_api_key(app.handle());
            #[cfg(target_os = "windows")]
            text_inserter::set_hold_to_talk_enabled(loaded.hold_to_talk);
            let state = app.state::<Arc<AppState>>();
            let mut settings = state
                .settings
                .lock()
                .map_err(|_| anyhow!("Failed to lock settings during setup"))?;
            *settings = loaded;

            if let Err(e) = setup_tray(app) {
                eprintln!("Failed to create tray icon: {e}");
            }

            // The frontend's first layout commit performs the initial reveal.
            // Until then the window has no authoritative logical size or DPI
            // placement and must remain hidden.

            #[cfg(target_os = "windows")]
            {
                let (wake, woken) = std::sync::mpsc::channel();
                std::thread::spawn(move || text_inserter::install_input_hooks(wake));

                // Sleeps until the hook thread reports a shortcut or a focus
                // change, so an idle app does no work here.
                let shortcut_handle = app.handle().clone();
                std::thread::spawn(move || {
                    let mut last_hook_log = std::time::Instant::now();
                    let mut last_hook_event = String::new();
                    while woken.recv().is_ok() {
                        // One key press can send several wake-ups; the flags
                        // below already hold all of them.
                        while woken.try_recv().is_ok() {}
                        let should_start = text_inserter::take_press_to_talk_start();
                        let should_stop = text_inserter::take_press_to_talk_stop();
                        let should_toggle = text_inserter::take_toggle_recording();
                        let debug_events = text_inserter::take_debug_events();
                        if debug_events != 0 {
                            let desc = text_inserter::describe_debug_events(debug_events);
                            let now = std::time::Instant::now();
                            let event_changed = desc != last_hook_event;
                            let throttle_elapsed = now.duration_since(last_hook_log).as_secs() >= 3;
                            if event_changed || throttle_elapsed {
                                debug_log_line(
                                    "hook",
                                    &format!(
                                        "events={} start_pending={} stop_pending={} toggle_pending={}",
                                        desc, should_start, should_stop, should_toggle
                                    ),
                                );
                                last_hook_log = now;
                                last_hook_event = desc;
                            }
                        }
                        if should_start {
                            debug_log_line("shortcut", "emit press-to-talk-start");
                            text_inserter::track_foreground();
                            reveal_overlay_window(&shortcut_handle);
                            let _ = shortcut_handle.emit("press-to-talk-start", ());
                        }
                        if should_stop {
                            debug_log_line("shortcut", "emit press-to-talk-stop");
                            let _ = shortcut_handle.emit("press-to-talk-stop", ());
                        }
                        if should_toggle {
                            debug_log_line("shortcut", "emit toggle-recording");
                            text_inserter::track_foreground();
                            reveal_overlay_window(&shortcut_handle);
                            let _ = shortcut_handle.emit("toggle-recording", ());
                        }
                        if let Some(name) = text_inserter::foreground_app_change() {
                            let _ = shortcut_handle.emit("foreground-app-changed", name);
                        }
                    }
                });

                // Moves the pill to the monitor the cursor is on. It runs on
                // its own thread so a monitor move never delays a shortcut.
                let monitor_handle = app.handle().clone();
                std::thread::spawn(move || {
                    let mut monitor_transition = MonitorTransitionDebouncer::default();
                    loop {
                        if let Some(cursor_anchor) = cursor_overlay_anchor() {
                            let current_anchor = monitor_handle
                                .state::<Arc<AppState>>()
                                .overlay_anchor
                                .lock()
                                .ok()
                                .and_then(|guard| *guard);

                            // Require two consecutive 100 ms observations before
                            // gathering and committing the destination geometry.
                            if let Some(target) =
                                monitor_transition.observe(current_anchor, cursor_anchor)
                            {
                                match sync_overlay_window_to_anchor(&monitor_handle, target) {
                                    Ok(true) => monitor_transition.reset(),
                                    Ok(false) => {}
                                    Err(err) => debug_log_line(
                                        "window",
                                        &format!("monitor switch failed: {err}"),
                                    ),
                                }
                            }
                        }
                        std::thread::sleep(MONITOR_FOLLOW_INTERVAL);
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_get_settings,
            app_get_openai_api_key,
            app_save_settings,
            worker_ping,
            worker_start_session,
            worker_stop_session,
            worker_append_audio_chunk,
            worker_finalize_session_audio,
            recording_begin,
            recording_append_chunk,
            recording_import,
            recording_transcribe,
            recording_finish,
            recording_list,
            recording_open,
            recordings_open_folder,
            debug_log,
            get_foreground_app,
            open_api_keys_page,
            capture_paste_target,
            copy_to_clipboard,
            type_to_target,
            insert_final_text,
            overlay_apply_layout
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
