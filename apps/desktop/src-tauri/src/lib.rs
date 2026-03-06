mod worker_client;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State,
};
use tokio::sync::Mutex;
use worker_client::WorkerClient;

// ---------------------------------------------------------------------------
// Platform: text insertion via clipboard + simulated Ctrl+V
// ---------------------------------------------------------------------------
#[cfg(target_os = "windows")]
mod text_inserter {
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

    static TARGET_HWND: AtomicIsize = AtomicIsize::new(0);
    static HOOK_HANDLE: AtomicIsize = AtomicIsize::new(0);
    static WIN_KEY_HELD: AtomicBool = AtomicBool::new(false);
    static SHORTCUT_FIRED: AtomicBool = AtomicBool::new(false);

    const INPUT_KEYBOARD: u32 = 1;
    const KEYEVENTF_KEYUP: u32 = 0x0002;
    const VK_CONTROL: u16 = 0x11;
    const VK_V: u16 = 0x56;

    const WH_KEYBOARD_LL: i32 = 13;
    const WM_KEYDOWN: u32 = 0x0100;
    const WM_KEYUP: u32 = 0x0101;
    const WM_SYSKEYDOWN: u32 = 0x0104;
    const WM_SYSKEYUP: u32 = 0x0105;
    const VK_LWIN: u32 = 0x5B;
    const VK_RWIN: u32 = 0x5C;
    const VK_S: u32 = 0x53;
    const LLKHF_INJECTED: u32 = 0x10;

    #[repr(C)]
    struct KbdInput {
        input_type: u32,
        _pad0: u32,
        w_vk: u16,
        w_scan: u16,
        dw_flags: u32,
        time: u32,
        _pad1: u32,
        dw_extra_info: usize,
        _union_pad: [u8; 8],
    }

    #[repr(C)]
    struct KbdLlHookStruct {
        vk_code: u32,
        scan_code: u32,
        flags: u32,
        time: u32,
        dw_extra_info: usize,
    }

    #[link(name = "user32")]
    extern "system" {
        fn SendInput(c_inputs: u32, p_inputs: *const KbdInput, cb_size: i32) -> u32;
        fn GetForegroundWindow() -> isize;
        fn SetForegroundWindow(h_wnd: isize) -> i32;
        fn GetWindowThreadProcessId(h_wnd: isize, lp_process_id: *mut u32) -> u32;
        fn SetWindowsHookExW(
            id_hook: i32,
            lpfn: unsafe extern "system" fn(i32, usize, isize) -> isize,
            h_mod: isize,
            dw_thread_id: u32,
        ) -> isize;
        fn CallNextHookEx(
            hhk: isize,
            n_code: i32,
            w_param: usize,
            l_param: isize,
        ) -> isize;
        fn GetMessageW(
            lp_msg: *mut u64,
            h_wnd: isize,
            w_msg_filter_min: u32,
            w_msg_filter_max: u32,
        ) -> i32;
        fn GetModuleHandleW(lp_module_name: *const u16) -> isize;
    }

    fn is_own_process(hwnd: isize) -> bool {
        let mut pid: u32 = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        pid == std::process::id()
    }

    /// Snapshot the current foreground window, ignoring our own process.
    pub fn save_foreground() {
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd != 0 && !is_own_process(hwnd) {
            TARGET_HWND.store(hwnd, Ordering::Relaxed);
        }
    }

    /// Returns `true` once if Win+S was pressed since the last call.
    pub fn take_shortcut_flag() -> bool {
        SHORTCUT_FIRED.swap(false, Ordering::SeqCst)
    }

    pub fn restore_and_paste() {
        let hwnd = TARGET_HWND.load(Ordering::Relaxed);
        if hwnd != 0 {
            unsafe { SetForegroundWindow(hwnd) };
            std::thread::sleep(std::time::Duration::from_millis(120));
        }
        unsafe {
            let size = std::mem::size_of::<KbdInput>() as i32;
            let inputs = [
                kbd(VK_CONTROL, 0),
                kbd(VK_V, 0),
                kbd(VK_V, KEYEVENTF_KEYUP),
                kbd(VK_CONTROL, KEYEVENTF_KEYUP),
            ];
            SendInput(4, inputs.as_ptr(), size);
        }
    }

    // ----- Low-level keyboard hook (intercepts Win+S before the shell) -----

    unsafe extern "system" fn ll_keyboard_proc(
        n_code: i32,
        w_param: usize,
        l_param: isize,
    ) -> isize {
        if n_code >= 0 {
            let info = &*(l_param as *const KbdLlHookStruct);

            if info.flags & LLKHF_INJECTED == 0 {
                let msg = w_param as u32;
                let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
                let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;

                match info.vk_code {
                    VK_LWIN | VK_RWIN => {
                        if down {
                            WIN_KEY_HELD.store(true, Ordering::SeqCst);
                        } else if up {
                            WIN_KEY_HELD.store(false, Ordering::SeqCst);
                        }
                    }
                    VK_S if down && WIN_KEY_HELD.load(Ordering::SeqCst) => {
                        SHORTCUT_FIRED.store(true, Ordering::SeqCst);
                        // Inject a harmless key so Windows counts a "combo"
                        // and won't open Start menu when Win is released.
                        let size = std::mem::size_of::<KbdInput>() as i32;
                        let noop = [kbd(0xFF, 0), kbd(0xFF, KEYEVENTF_KEYUP)];
                        SendInput(2, noop.as_ptr(), size);
                        return 1;
                    }
                    _ => {}
                }
            }
        }
        CallNextHookEx(
            HOOK_HANDLE.load(Ordering::Relaxed),
            n_code,
            w_param,
            l_param,
        )
    }

    /// Installs the low-level keyboard hook and runs a message pump.
    /// Blocks forever — must be called from a dedicated thread.
    pub fn install_keyboard_hook() {
        let h_mod = unsafe { GetModuleHandleW(std::ptr::null()) };
        let hook =
            unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, ll_keyboard_proc, h_mod, 0) };
        HOOK_HANDLE.store(hook, Ordering::Relaxed);

        let mut msg_buf = [0u64; 8];
        loop {
            let ret = unsafe { GetMessageW(msg_buf.as_mut_ptr(), 0, 0, 0) };
            if ret <= 0 {
                break;
            }
        }
    }

    fn kbd(vk: u16, flags: u32) -> KbdInput {
        KbdInput {
            input_type: INPUT_KEYBOARD,
            _pad0: 0,
            w_vk: vk,
            w_scan: 0,
            dw_flags: flags,
            time: 0,
            _pad1: 0,
            dw_extra_info: 0,
            _union_pad: [0; 8],
        }
    }
}

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

#[tauri::command]
async fn paste_to_target(text: String) -> Result<(), String> {
    arboard::Clipboard::new()
        .and_then(|mut c| c.set_text(&text))
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "windows")]
    text_inserter::restore_and_paste();

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

fn make_tray_icon() -> tauri::image::Image<'static> {
    const S: u32 = 32;
    let mut rgba = vec![0u8; (S * S * 4) as usize];
    let center = S as f32 / 2.0;
    let radius = center - 2.0;

    for y in 0..S {
        for x in 0..S {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let dist = (dx * dx + dy * dy).sqrt();
            let i = ((y * S + x) * 4) as usize;

            let alpha = if dist <= radius - 0.7 {
                255.0
            } else if dist <= radius + 0.7 {
                ((radius + 0.7 - dist) / 1.4 * 255.0).clamp(0.0, 255.0)
            } else {
                0.0
            };

            if alpha > 0.0 {
                rgba[i] = 0x22;
                rgba[i + 1] = 0xc5;
                rgba[i + 2] = 0x5e;
                rgba[i + 3] = alpha as u8;
            }
        }
    }

    let leaked: &'static [u8] = Box::leak(rgba.into_boxed_slice());
    tauri::image::Image::new(leaked, S, S)
}

fn setup_tray(app: &tauri::App) -> Result<()> {
    let show_i = MenuItem::with_id(app, "show", "Show / Hide", true, None::<&str>)
        .map_err(|e| anyhow!("{e}"))?;
    let record_i =
        MenuItem::with_id(app, "record", "Toggle Recording  (Win+S)", true, None::<&str>)
            .map_err(|e| anyhow!("{e}"))?;
    let sep =
        PredefinedMenuItem::separator(app).map_err(|e| anyhow!("{e}"))?;
    let quit_i = MenuItem::with_id(app, "quit", "Quit OpenWhisper", true, None::<&str>)
        .map_err(|e| anyhow!("{e}"))?;

    let menu = Menu::with_items(app, &[&show_i, &record_i, &sep, &quit_i])
        .map_err(|e| anyhow!("{e}"))?;

    TrayIconBuilder::new()
        .icon(make_tray_icon())
        .tooltip("OpenWhisper – Win+S to dictate")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    if w.is_visible().unwrap_or(false) {
                        let _ = w.hide();
                    } else {
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                }
            }
            "record" => {
                #[cfg(target_os = "windows")]
                text_inserter::save_foreground();
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
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
                if let Some(w) = app.get_webview_window("main") {
                    if w.is_visible().unwrap_or(false) {
                        let _ = w.hide();
                    } else {
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                }
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

            if let Err(e) = setup_tray(app) {
                eprintln!("Failed to create tray icon: {e}");
            }

            #[cfg(target_os = "windows")]
            {
                std::thread::spawn(|| text_inserter::install_keyboard_hook());

                let poll_handle = app.handle().clone();
                std::thread::spawn(move || loop {
                    text_inserter::save_foreground();
                    if text_inserter::take_shortcut_flag() {
                        text_inserter::save_foreground();
                        if let Some(w) = poll_handle.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                        let _ = poll_handle.emit("toggle-recording", ());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                });
            }

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
            worker_finalize_session_audio,
            paste_to_target
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

