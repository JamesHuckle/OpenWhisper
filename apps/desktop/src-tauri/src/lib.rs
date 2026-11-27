#[cfg(target_os = "windows")]
mod overlay_positioner;
// Writes dictated text into the field focused when recording began.
#[cfg(target_os = "windows")]
mod text_inserter;

use openwhisper_core::{
    api_key_store, debug_log, json_file, models, overlay_geometry, paste_target, recording_store,
    recordings, settings, sync, transcriber, transcription_sessions,
};

use anyhow::{anyhow, Result};
use debug_log::debug_log_line;
use models::{is_supported_transcription_model, migrate_transcription_model};
use overlay_geometry::{
    overlay_placement, MonitorTransitionDebouncer, OverlayAnchor, OverlayLayoutSpec,
    OverlayPlacement, OverlayRevealIntent, PlacementVisibility,
};
use paste_target::WriteOutcome;
use recording_store::PcmWavWriter;
use recordings::{
    close_active_recording, finish_recording, import_recording, list_recording_metadata, now_ms,
    read_recording_metadata, recording_parts_path, recover_interrupted, write_recording_metadata,
    ActiveRecording, RecordingMetadata, RecordingStartResponse,
};
use serde::{Deserialize, Serialize};
use settings::{frontend_settings_from, AppSettings, FrontendSettings};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use std::borrow::Cow;
use sync::lock;
use tauri::{
    ipc::{InvokeBody, Request},
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, LogicalSize, Manager, Size, State, Window, WindowEvent,
};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use transcriber::SavedAudio;
use transcription_sessions::{opens_live_session, FinalizeAudio, Sessions, SESSION_EVENT};
use uuid::Uuid;

/// How often the pill checks which monitor the cursor is on while it is
/// moving to another one.
#[cfg(target_os = "windows")]
const MONITOR_FOLLOW_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// The longest the pill waits between checks when the mouse hook reports no
/// monitor change: it catches display changes the hook cannot see (a new
/// resolution or scaling, a monitor plugged in or removed).
#[cfg(target_os = "windows")]
const MONITOR_FALLBACK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// How long the pill waits after a failed move to another monitor before
/// it tries again.
#[cfg(target_os = "windows")]
const MONITOR_RETRY_AFTER_FAILURE: std::time::Duration = std::time::Duration::from_secs(1);

struct AppState {
    // Set once the app starts. Each dictation is a session here; none waits
    // for another, so Cancel and settings saves never queue behind one.
    transcription: OnceLock<Sessions>,
    settings: std::sync::Mutex<AppSettings>,
    overlay_anchor: std::sync::Mutex<Option<OverlayAnchor>>,
    overlay_layout: std::sync::Mutex<Option<OverlayLayoutSpec>>,
    // Held while the pill is placed. A page layout waits for it; a monitor
    // move skips its turn and tries again.
    placing: tokio::sync::Mutex<()>,
    // Microphone recordings in progress, by recording id. Each keeps its
    // file open until the recording is transcribed or finished.
    recordings: std::sync::Mutex<HashMap<String, ActiveRecording>>,
}

fn resolve_overlay_anchor(state: &AppState) -> Option<OverlayAnchor> {
    let mut guard = lock(&state.overlay_anchor);

    #[cfg(target_os = "windows")]
    if guard.is_none() {
        if let Some(anchor) = overlay_positioner::cursor_monitor_anchor()
            .or_else(overlay_positioner::primary_monitor_anchor)
        {
            *guard = Some(anchor);
        }
    }

    *guard
}

/// The pill's monitor, and where the pill goes on it, once the page has
/// committed a layout.
fn current_placement(state: &AppState) -> Option<(OverlayAnchor, OverlayPlacement)> {
    let anchor = resolve_overlay_anchor(state)?;
    let layout = (*lock(&state.overlay_layout))?;
    Some((anchor, overlay_placement(anchor, layout)))
}

#[cfg(target_os = "windows")]
fn sync_overlay_window_to_anchor(app: &AppHandle, anchor: OverlayAnchor) -> Result<bool, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let Ok(_placing) = state.placing.try_lock() else {
        return Ok(false);
    };

    let previous = *lock(&state.overlay_anchor);
    if previous == Some(anchor) {
        return Ok(true);
    }
    let Some(layout) = *lock(&state.overlay_layout) else {
        return Ok(false);
    };
    let Some(window) = app.get_webview_window("main") else {
        return Ok(false);
    };
    let hwnd = window.hwnd().map_err(|e| e.to_string())?.0 as isize;
    let placement = overlay_placement(anchor, layout);
    let was_visible = window.is_visible().map_err(|e| e.to_string())?;

    // A pill that was visible comes back at its last good placement if the
    // move fails, so a failed move never hides it for good.
    let restore = was_visible
        .then(|| previous.map_or(placement, |previous| overlay_placement(previous, layout)));
    let moved = overlay_positioner::move_hidden(hwnd, placement, restore);
    if was_visible {
        overlay_positioner::reassert_topmost(hwnd);
    }
    let correction_count = moved?;
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

    *lock(&state.overlay_anchor) = Some(anchor);
    Ok(true)
}

#[cfg(target_os = "windows")]
fn position_main_window_before_reveal(app: &AppHandle) -> Result<bool, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let Some(window) = app.get_webview_window("main") else {
        return Ok(false);
    };
    let Some((_, placement)) = current_placement(&state) else {
        return Ok(false);
    };
    overlay_positioner::set_window_placement(
        window.hwnd().map_err(|e| e.to_string())?.0 as isize,
        placement,
        PlacementVisibility::Keep,
    )?;
    Ok(true)
}

#[derive(Debug, Serialize)]
struct SessionStartResponse {
    session_id: String,
    /// The window streams audio only to a Live session.
    live: bool,
}

fn recordings_root(app_handle: &AppHandle) -> Result<PathBuf, String> {
    let mut root = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data directory: {e}"))?;
    root.push("recordings");
    Ok(root)
}

/// Opens a file or folder in the program the OS picks for it.
fn open_path(app_handle: &AppHandle, path: &Path) -> Result<(), String> {
    app_handle
        .opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
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
    let root = recordings_root(&app_handle)?;
    // One microphone recording runs at a time. Close any the UI left open
    // (for example after a reload) so their files are complete.
    let left_open: Vec<String> = lock(&state.recordings).keys().cloned().collect();
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
    lock(&state.recordings).insert(id.clone(), ActiveRecording { writer, metadata });
    // Finishing or importing a recording prunes old ones, not starting it.
    Ok(RecordingStartResponse { id })
}

/// Reads an audio chunk the page sends as raw bytes, with the id of its
/// recording or session in a header. Raw bytes skip the base64 text a JSON
/// argument needs, so a chunk is never encoded on the page's main thread.
/// If the webview blocks Tauri's IPC protocol, Tauri sends the same bytes as
/// a JSON list of numbers instead, which is read here too.
fn audio_chunk_request<'a>(
    request: &'a Request<'_>,
    id_header: &str,
) -> Result<(&'a str, Cow<'a, [u8]>), String> {
    let id = request
        .headers()
        .get(id_header)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| format!("The audio chunk has no {id_header} header"))?;
    let chunk = match request.body() {
        InvokeBody::Raw(bytes) => Cow::Borrowed(bytes.as_slice()),
        InvokeBody::Json(value) => Cow::Owned(
            Vec::<u8>::deserialize(value).map_err(|e| format!("Invalid audio chunk: {e}"))?,
        ),
    };
    Ok((id, chunk))
}

/// Appends audio to an open recording's WAV file. Its `metadata.json` is not
/// rewritten while it records: the recordings list reads the size of an open
/// recording from its file, and closing the recording saves the final size.
#[tauri::command(async)]
fn recording_append_chunk(
    request: Request<'_>,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let (recording_id, chunk) = audio_chunk_request(&request, "recording-id")?;
    let mut recordings = lock(&state.recordings);
    let active = recordings
        .get_mut(recording_id)
        .ok_or_else(|| "This recording is no longer being saved".to_string())?;
    active
        .writer
        .append(&chunk, Instant::now())
        .map_err(|e| e.to_string())?;
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
    let root = recordings_root(&app_handle)?;
    // A large file copies on a blocking thread, not an async worker.
    tauri::async_runtime::spawn_blocking(move || import_recording(&root, &source))
        .await
        .map_err(|e| e.to_string())?
        .map(Some)
}

/// Transcribes a saved recording in a session. The transcriber reads the
/// file from disk, so its audio never passes through the UI. The result
/// arrives as the session's final or error event.
#[tauri::command]
async fn recording_transcribe(
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
    recording_id: String,
    session_id: String,
) -> Result<(), String> {
    let sessions = transcription(&state)?;
    let root = recordings_root(&app_handle)?;
    close_active_recording(&state.recordings, &root, &recording_id)?;
    let metadata = read_recording_metadata(&root, &recording_id).map_err(|e| e.to_string())?;
    let audio_path = root.join(&recording_id).join(&metadata.file_name);
    let microphone = metadata.source == "microphone";
    if microphone {
        // A recording the app did not finish (it stopped while recording)
        // has a header up to a second behind its audio. It is repaired
        // before the session checks the recording for speech.
        recording_store::repair_wav_header(&audio_path, recording_store::MICROPHONE_SAMPLE_RATE)
            .map_err(|e| format!("Could not read saved recording: {e}"))?;
    }
    let audio = SavedAudio {
        audio_path,
        mime_type: metadata.mime_type,
        microphone,
        // A long recording's finished parts are kept here, so a retry after
        // a failure sends only the rest. Removed once it completes.
        parts_path: Some(recording_parts_path(&root, &recording_id)),
    };
    sessions
        .finalize(&session_id, FinalizeAudio::Saved(audio))
        .await
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
    let root = recordings_root(&app_handle)?;
    close_active_recording(&state.recordings, &root, &recording_id)?;
    finish_recording(&root, &recording_id, status, transcript, error).map_err(|e| e.to_string())
}

#[tauri::command(async)]
fn recording_list(app_handle: AppHandle) -> Result<Vec<RecordingMetadata>, String> {
    let root = recordings_root(&app_handle)?;
    list_recording_metadata(&root).map_err(|e| e.to_string())
}

#[tauri::command]
fn recording_open(app_handle: AppHandle, recording_id: String) -> Result<(), String> {
    let root = recordings_root(&app_handle)?;
    let metadata = read_recording_metadata(&root, &recording_id).map_err(|e| e.to_string())?;
    let audio_path = root.join(recording_id).join(metadata.file_name);
    open_path(&app_handle, &audio_path)
}

#[tauri::command]
fn recordings_open_folder(app_handle: AppHandle) -> Result<(), String> {
    let root = recordings_root(&app_handle)?;
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    open_path(&app_handle, &root)
}

/// Opens the folder holding `openwhisper.log` (src/debug_log.rs), so a user
/// can send it when something went wrong.
#[tauri::command]
fn logs_open_folder(app_handle: AppHandle) -> Result<(), String> {
    let dir = app_handle.path().app_log_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    open_path(&app_handle, &dir)
}

fn transcription(state: &AppState) -> Result<&Sessions, String> {
    state
        .transcription
        .get()
        .ok_or_else(|| "The transcriber did not start".to_string())
}

#[tauri::command]
async fn app_get_settings(state: State<'_, Arc<AppState>>) -> Result<FrontendSettings, String> {
    let settings = lock(&state.settings).clone();
    Ok(frontend_settings_from(&settings))
}

#[tauri::command]
async fn app_get_openai_api_key(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    let settings = lock(&state.settings).clone();
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
    let _saving = lock(&SAVING_SETTINGS);
    let mut previous_key = None;
    let updated = {
        let mut current = lock(&state.settings).clone();
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

    *lock(&state.settings) = updated.clone();

    #[cfg(target_os = "windows")]
    text_inserter::set_hold_to_talk_enabled(updated.hold_to_talk);

    // The next dictation reads the new settings when it starts.
    Ok(frontend_settings_from(&updated))
}

// The session commands keep their worker_ names so the window's calls stay
// the same; the work is done here, in the app.

#[tauri::command]
async fn worker_start_session(
    state: State<'_, Arc<AppState>>,
    standard_only: Option<bool>,
) -> Result<SessionStartResponse, String> {
    // The session keeps the settings saved at this moment, so a saved change
    // applies to the next dictation. The lock is released here, before the
    // Live connection is awaited.
    let settings = lock(&state.settings).clone();
    // Saved audio, and a Live dictation whose connection failed, are
    // transcribed from the file with Standard; no Live connection is opened.
    let standard_only = standard_only.unwrap_or(false);
    let live = opens_live_session(&settings, standard_only);
    let session_id = transcription(&state)?
        .start(settings, standard_only)
        .await?;
    Ok(SessionStartResponse { session_id, live })
}

/// Stops a session at once, even while it is being transcribed.
#[tauri::command]
async fn worker_stop_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<(), String> {
    transcription(&state)?.stop(&session_id)
}

/// Streams a Live recording's audio chunk, sent by the page as raw bytes, to
/// the session's Realtime connection.
#[tauri::command]
async fn worker_append_audio_chunk(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<(), String> {
    let (session_id, chunk) = audio_chunk_request(&request, "session-id")?;
    transcription(&state)?.append(&session_id, &chunk).await
}

#[tauri::command]
async fn worker_finalize_session_audio(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<(), String> {
    transcription(&state)?
        .finalize(&session_id, FinalizeAudio::Live)
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

/// Type one live piece of the transcript into the locked field.
#[tauri::command]
async fn type_to_target(text: String) -> Result<WriteOutcome, String> {
    #[cfg(target_os = "windows")]
    {
        tauri::async_runtime::spawn_blocking(move || text_inserter::type_live(&text))
            .await
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = text;
        Ok(WriteOutcome::Stopped(paste_target::Blocked::Unsupported))
    }
}

/// Write the final transcript into the locked field, replacing the live
/// draft only when it is safe to delete.
#[tauri::command]
async fn insert_final_text(text: String) -> Result<WriteOutcome, String> {
    #[cfg(target_os = "windows")]
    {
        tauri::async_runtime::spawn_blocking(move || text_inserter::insert_final(&text))
            .await
            .map_err(|e| e.to_string())?
    }
    #[cfg(not(target_os = "windows"))]
    {
        copy_to_clipboard(text)?;
        Ok(WriteOutcome::Copied(paste_target::Blocked::Unsupported))
    }
}

#[tauri::command]
async fn overlay_apply_layout(
    window: Window,
    state: State<'_, Arc<AppState>>,
    width: f64,
    height: f64,
    visible: bool,
) -> Result<(), String> {
    let app_state = state.inner().clone();

    #[cfg(target_os = "windows")]
    {
        let _placing = app_state.placing.lock().await;
        *lock(&app_state.overlay_layout) = Some(OverlayLayoutSpec { width, height });

        if let Some((anchor, placement)) = current_placement(&app_state) {
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
            if visible && !was_visible {
                // A hidden pill is shown only once Windows has settled it at
                // its placement. The checks sleep (at most about 100 ms), so
                // they run off the async runtime's threads.
                tauri::async_runtime::spawn_blocking(move || {
                    overlay_positioner::settle_hidden_at(hwnd, placement)?
                        .ok_or_else(|| "Initial overlay placement did not settle".to_string())?;
                    overlay_positioner::set_window_placement(
                        hwnd,
                        placement,
                        PlacementVisibility::Show,
                    )
                })
                .await
                .map_err(|e| e.to_string())??;
            } else {
                let visibility = if visible {
                    PlacementVisibility::Keep
                } else {
                    PlacementVisibility::HideWithoutRedraw
                };
                overlay_positioner::set_window_placement(hwnd, placement, visibility)?;
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
    settings::save_settings_file(&settings_path(app_handle)?, settings)
}

fn make_tray_icon() -> tauri::image::Image<'static> {
    let bytes = include_bytes!("../icons/32x32.png");
    tauri::image::Image::from_bytes(bytes)
        .expect("tray icon 32x32.png")
        .to_owned()
}

fn reveal_window(app: &AppHandle, intent: OverlayRevealIntent) {
    if matches!(intent, OverlayRevealIntent::Passive) {
        if !lock(&app.state::<Arc<AppState>>().settings).show_idle_pill {
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

fn setup_tray(app: &tauri::App) -> Result<()> {
    let show_i = MenuItem::with_id(app, "show", "Show Pill", true, None::<&str>)?;
    let record_i = MenuItem::with_id(app, "record", "Toggle Recording", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit_i = MenuItem::with_id(app, "quit", "Quit OpenWhisper", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&show_i, &record_i, &sep, &quit_i])?;

    TrayIconBuilder::new()
        .icon(make_tray_icon())
        .tooltip("OpenWhisper – Ctrl+Space to talk")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => reveal_window(app, OverlayRevealIntent::Interactive),
            "record" => {
                #[cfg(target_os = "windows")]
                text_inserter::track_foreground();
                reveal_window(app, OverlayRevealIntent::Interactive);
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
                reveal_window(&app, OverlayRevealIntent::Interactive);
            }
        })
        .build(app)?;

    Ok(())
}

pub fn run() {
    let app_state = Arc::new(AppState {
        transcription: OnceLock::new(),
        settings: std::sync::Mutex::new(AppSettings::default()),
        overlay_anchor: std::sync::Mutex::new(None),
        overlay_layout: std::sync::Mutex::new(None),
        placing: tokio::sync::Mutex::new(()),
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
                    reveal_window(&window.app_handle(), OverlayRevealIntent::Passive);
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
            // First, so every later line also reaches the log file.
            match app.path().app_log_dir() {
                Ok(dir) => debug_log::init_log_file(&dir),
                Err(error) => debug_log_line("log", &format!("no log folder: {error}")),
            }
            // Before anything can start a recording or a transcription: setup
            // runs on the main thread, so no command is handled until it ends.
            if let Err(error) = recordings_root(app.handle())
                .and_then(|root| recover_interrupted(&root).map_err(|e| e.to_string())) {
                debug_log_line("recording", &format!("recovery failed: {error}"));
            }
            let emit_app = app.handle().clone();
            match Sessions::new(Arc::new(move |event| {
                let _ = emit_app.emit(SESSION_EVENT, event);
            })) {
                Ok(sessions) => {
                    let _ = app.state::<Arc<AppState>>().transcription.set(sessions);
                }
                Err(error) => debug_log_line("transcriber", &error),
            }
            let loaded = load_settings_and_api_key(app.handle());
            #[cfg(target_os = "windows")]
            text_inserter::set_hold_to_talk_enabled(loaded.hold_to_talk);
            *lock(&app.state::<Arc<AppState>>().settings) = loaded;

            if let Err(e) = setup_tray(app) {
                debug_log_line("tray", &format!("Failed to create tray icon: {e}"));
            }

            // The frontend's first layout commit performs the initial reveal.
            // Until then the window has no authoritative logical size or DPI
            // placement and must remain hidden.

            #[cfg(target_os = "windows")]
            {
                let (wake, woken) = std::sync::mpsc::channel();
                let (monitor_wake, monitor_woken) = std::sync::mpsc::channel::<()>();
                std::thread::spawn(move || {
                    text_inserter::install_input_hooks(wake, monitor_wake)
                });

                // Sleeps until the hook thread reports a shortcut, so an idle
                // app does no work here.
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
                            reveal_window(&shortcut_handle, OverlayRevealIntent::Passive);
                            let _ = shortcut_handle.emit("press-to-talk-start", ());
                        }
                        if should_stop {
                            debug_log_line("shortcut", "emit press-to-talk-stop");
                            let _ = shortcut_handle.emit("press-to-talk-stop", ());
                        }
                        if should_toggle {
                            debug_log_line("shortcut", "emit toggle-recording");
                            text_inserter::track_foreground();
                            reveal_window(&shortcut_handle, OverlayRevealIntent::Passive);
                            let _ = shortcut_handle.emit("toggle-recording", ());
                        }
                    }
                });

                // Moves the pill to the monitor the cursor is on. It runs on
                // its own thread so a monitor move never delays a shortcut.
                // It sleeps until the mouse hook sees the cursor enter another
                // monitor, or for at most MONITOR_FALLBACK_INTERVAL, so an
                // idle app does almost no work here.
                let monitor_handle = app.handle().clone();
                std::thread::spawn(move || {
                    use std::sync::mpsc::RecvTimeoutError;
                    let mut monitor_transition = MonitorTransitionDebouncer::default();
                    loop {
                        if let Err(RecvTimeoutError::Disconnected) =
                            monitor_woken.recv_timeout(MONITOR_FALLBACK_INTERVAL)
                        {
                            // The hook thread has gone: check on a timer instead.
                            std::thread::sleep(MONITOR_FOLLOW_INTERVAL);
                        }
                        // Crossing several monitors sends several wake-ups;
                        // one check below covers them all.
                        while monitor_woken.try_recv().is_ok() {}

                        monitor_transition.reset();
                        // Follow the cursor until the pill is on its monitor.
                        while let Some(cursor_anchor) = overlay_positioner::cursor_monitor_anchor() {
                            let current_anchor =
                                *lock(&monitor_handle.state::<Arc<AppState>>().overlay_anchor);
                            if current_anchor == Some(cursor_anchor) {
                                break;
                            }

                            // Require two consecutive 100 ms observations before
                            // gathering and committing the destination geometry.
                            if let Some(target) =
                                monitor_transition.observe(current_anchor, cursor_anchor)
                            {
                                match sync_overlay_window_to_anchor(&monitor_handle, target) {
                                    Ok(true) => break,
                                    Ok(false) => {}
                                    Err(err) => {
                                        debug_log_line(
                                            "window",
                                            &format!("monitor switch failed: {err}"),
                                        );
                                        // A failed move shows the pill again where
                                        // it was; wait before the next try so it
                                        // does not flicker between attempts.
                                        std::thread::sleep(MONITOR_RETRY_AFTER_FAILURE);
                                    }
                                }
                            }
                            std::thread::sleep(MONITOR_FOLLOW_INTERVAL);
                        }
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_get_settings,
            app_get_openai_api_key,
            app_save_settings,
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
            logs_open_folder,
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
