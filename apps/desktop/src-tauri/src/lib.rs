mod worker_client;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, Position, Size,
    State, Window, WindowEvent,
};
use tauri_plugin_opener::OpenerExt;
use tokio::sync::Mutex;
use worker_client::WorkerClient;

// ---------------------------------------------------------------------------
// Platform: text insertion via clipboard + simulated Ctrl+V
// ---------------------------------------------------------------------------
#[cfg(target_os = "windows")]
mod text_inserter {
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};

    static TARGET_HWND: AtomicIsize = AtomicIsize::new(0);
    static HOOK_HANDLE: AtomicIsize = AtomicIsize::new(0);
    static CTRL_HELD: AtomicBool = AtomicBool::new(false);
    static PRESS_TO_TALK_ACTIVE: AtomicBool = AtomicBool::new(false);
    static SPACE_SWALLOWED: AtomicBool = AtomicBool::new(false);
    static PRESS_TO_TALK_START: AtomicBool = AtomicBool::new(false);
    static PRESS_TO_TALK_STOP: AtomicBool = AtomicBool::new(false);
    static DEBUG_EVENT_BITS: AtomicU32 = AtomicU32::new(0);

    const INPUT_KEYBOARD: u32 = 1;
    const KEYEVENTF_KEYUP: u32 = 0x0002;
    const VK_CONTROL: u16 = 0x11;
    const VK_V: u16 = 0x56;

    const WH_KEYBOARD_LL: i32 = 13;
    const WM_KEYDOWN: u32 = 0x0100;
    const WM_KEYUP: u32 = 0x0101;
    const WM_SYSKEYDOWN: u32 = 0x0104;
    const WM_SYSKEYUP: u32 = 0x0105;
    const VK_CONTROL_CODE: u32 = 0x11;
    const VK_LCONTROL_CODE: u32 = 0xA2;
    const VK_RCONTROL_CODE: u32 = 0xA3;
    const VK_SPACE: u32 = 0x20;
    const LLKHF_INJECTED: u32 = 0x10;
    const DBG_CTRL_DOWN: u32 = 1 << 0;
    const DBG_CTRL_UP: u32 = 1 << 1;
    const DBG_SPACE_DOWN: u32 = 1 << 2;
    const DBG_SPACE_REPEAT: u32 = 1 << 3;
    const DBG_SPACE_UP: u32 = 1 << 4;
    const DBG_START_FLAG: u32 = 1 << 5;
    const DBG_STOP_BY_CTRL: u32 = 1 << 6;
    const DBG_STOP_BY_SPACE: u32 = 1 << 7;

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
        fn CallNextHookEx(hhk: isize, n_code: i32, w_param: usize, l_param: isize) -> isize;
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

    /// Returns `true` once when press-to-talk should start.
    pub fn take_press_to_talk_start() -> bool {
        PRESS_TO_TALK_START.swap(false, Ordering::SeqCst)
    }

    /// Returns `true` once when press-to-talk should stop.
    pub fn take_press_to_talk_stop() -> bool {
        PRESS_TO_TALK_STOP.swap(false, Ordering::SeqCst)
    }

    pub fn take_debug_events() -> u32 {
        DEBUG_EVENT_BITS.swap(0, Ordering::SeqCst)
    }

    pub fn describe_debug_events(bits: u32) -> String {
        let mut events = Vec::new();
        if bits & DBG_CTRL_DOWN != 0 {
            events.push("ctrl_down");
        }
        if bits & DBG_CTRL_UP != 0 {
            events.push("ctrl_up");
        }
        if bits & DBG_SPACE_DOWN != 0 {
            events.push("space_down");
        }
        if bits & DBG_SPACE_REPEAT != 0 {
            events.push("space_repeat");
        }
        if bits & DBG_SPACE_UP != 0 {
            events.push("space_up");
        }
        if bits & DBG_START_FLAG != 0 {
            events.push("start_flag");
        }
        if bits & DBG_STOP_BY_CTRL != 0 {
            events.push("stop_flag_ctrl_up");
        }
        if bits & DBG_STOP_BY_SPACE != 0 {
            events.push("stop_flag_space_up");
        }
        events.join(",")
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

    // ----- Low-level keyboard hook (Ctrl+Space hold-to-talk) -----

    // The hook proc must stay extremely small to avoid Windows timing
    // it out. It only flips atomics and swallows Space while the
    // Ctrl+Space press-to-talk gesture is active.
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
                    VK_CONTROL_CODE | VK_LCONTROL_CODE | VK_RCONTROL_CODE => {
                        if down {
                            CTRL_HELD.store(true, Ordering::SeqCst);
                            DEBUG_EVENT_BITS.fetch_or(DBG_CTRL_DOWN, Ordering::SeqCst);
                        } else if up {
                            CTRL_HELD.store(false, Ordering::SeqCst);
                            DEBUG_EVENT_BITS.fetch_or(DBG_CTRL_UP, Ordering::SeqCst);
                            if PRESS_TO_TALK_ACTIVE.swap(false, Ordering::SeqCst) {
                                PRESS_TO_TALK_STOP.store(true, Ordering::SeqCst);
                                DEBUG_EVENT_BITS.fetch_or(DBG_STOP_BY_CTRL, Ordering::SeqCst);
                            }
                        }
                    }
                    VK_SPACE => {
                        if down && CTRL_HELD.load(Ordering::SeqCst) {
                            SPACE_SWALLOWED.store(true, Ordering::SeqCst);
                            DEBUG_EVENT_BITS.fetch_or(DBG_SPACE_DOWN, Ordering::SeqCst);
                            if !PRESS_TO_TALK_ACTIVE.swap(true, Ordering::SeqCst) {
                                PRESS_TO_TALK_START.store(true, Ordering::SeqCst);
                                DEBUG_EVENT_BITS.fetch_or(DBG_START_FLAG, Ordering::SeqCst);
                            }
                            return 1;
                        }

                        if SPACE_SWALLOWED.load(Ordering::SeqCst) && down {
                            DEBUG_EVENT_BITS.fetch_or(DBG_SPACE_REPEAT, Ordering::SeqCst);
                            return 1;
                        }

                        if SPACE_SWALLOWED.swap(false, Ordering::SeqCst) && up {
                            DEBUG_EVENT_BITS.fetch_or(DBG_SPACE_UP, Ordering::SeqCst);
                            if PRESS_TO_TALK_ACTIVE.swap(false, Ordering::SeqCst) {
                                PRESS_TO_TALK_STOP.store(true, Ordering::SeqCst);
                                DEBUG_EVENT_BITS.fetch_or(DBG_STOP_BY_SPACE, Ordering::SeqCst);
                            }
                            return 1;
                        }
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

    /// Installs the low-level keyboard hook and runs a blocking message pump.
    /// Blocks forever — must be called from a dedicated thread.
    pub fn install_keyboard_hook() {
        let h_mod = unsafe { GetModuleHandleW(std::ptr::null()) };
        let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, ll_keyboard_proc, h_mod, 0) };
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

#[cfg(target_os = "windows")]
mod overlay_positioner {
    use std::mem::size_of;

    const MONITOR_DEFAULTTOPRIMARY: u32 = 1;
    const OVERLAY_BOTTOM_MARGIN: i32 = 10;

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    struct MonitorInfo {
        cb_size: u32,
        rc_monitor: Rect,
        rc_work: Rect,
        dw_flags: u32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn MonitorFromPoint(point: Point, dw_flags: u32) -> isize;
        fn GetMonitorInfoW(h_monitor: isize, lpmi: *mut MonitorInfo) -> i32;
    }

    pub fn primary_monitor_anchor() -> Option<(i32, i32)> {
        let monitor = unsafe {
            MonitorFromPoint(
                Point { x: 0, y: 0 },
                MONITOR_DEFAULTTOPRIMARY,
            )
        };
        if monitor == 0 {
            return None;
        }

        let mut info = MonitorInfo {
            cb_size: size_of::<MonitorInfo>() as u32,
            rc_monitor: Rect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            rc_work: Rect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            dw_flags: 0,
        };

        if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
            return None;
        }

        Some((
            (info.rc_work.left + info.rc_work.right) / 2,
            info.rc_work.bottom - OVERLAY_BOTTOM_MARGIN,
        ))
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct AppSettings {
    openai_api_key: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FrontendSettings {
    has_openai_api_key: bool,
    openai_api_key_preview: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct OverlayAnchor {
    center_x: i32,
    bottom_y: i32,
}

#[derive(Debug, Clone, Copy)]
struct OverlayFrame {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl OverlayFrame {
    fn matches(&self, x: i32, y: i32, width: i32, height: i32) -> bool {
        const TOLERANCE: i32 = 2;

        (self.x - x).abs() <= TOLERANCE
            && (self.y - y).abs() <= TOLERANCE
            && (self.width - width).abs() <= TOLERANCE
            && (self.height - height).abs() <= TOLERANCE
    }
}

struct AppState {
    worker: Mutex<Option<WorkerClient>>,
    settings: std::sync::Mutex<AppSettings>,
    overlay_anchor: std::sync::Mutex<Option<OverlayAnchor>>,
    overlay_layout_locked: AtomicBool,
    overlay_expected_frame: std::sync::Mutex<Option<OverlayFrame>>,
}

fn frontend_settings_from(settings: &AppSettings) -> FrontendSettings {
    let openai_api_key_preview = masked_api_key_preview(&settings.openai_api_key);
    FrontendSettings {
        has_openai_api_key: openai_api_key_preview.is_some(),
        openai_api_key_preview,
    }
}

fn masked_api_key_preview(api_key: &str) -> Option<String> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return None;
    }

    let prefix: String = trimmed.chars().take(4).collect();
    let total_chars = trimmed.chars().count();
    if total_chars <= 4 {
        Some(prefix)
    } else {
        Some(format!("{prefix}{}", "*".repeat(8)))
    }
}

#[derive(Debug, Serialize)]
struct SessionStartResponse {
    session_id: String,
}

fn debug_log_line(source: &str, message: &str) {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    eprintln!("[openwhisper][{now_ms}][{source}] {message}");
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
    openai_api_key: String,
) -> Result<FrontendSettings, String> {
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
fn debug_log(message: String) -> Result<(), String> {
    debug_log_line("frontend", &message);
    Ok(())
}

#[tauri::command]
fn open_api_keys_page(app_handle: AppHandle) -> Result<(), String> {
    app_handle
        .opener()
        .open_url("https://platform.openai.com/api-keys", None::<&str>)
        .map_err(|e| e.to_string())
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

#[tauri::command]
fn overlay_apply_layout(
    window: Window,
    state: State<'_, Arc<AppState>>,
    width: f64,
    height: f64,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let anchor = {
            let mut guard = state
                .overlay_anchor
                .lock()
                .map_err(|_| "Failed to lock overlay anchor".to_string())?;

            if guard.is_none() {
                if let Some((center_x, bottom_y)) =
                    overlay_positioner::primary_monitor_anchor()
                {
                    *guard = Some(OverlayAnchor { center_x, bottom_y });
                }
            }

            *guard
        };

        if let Some(anchor) = anchor {
            let scale = window.scale_factor().map_err(|e| e.to_string())?;
            let physical_width = (width * scale).round() as i32;
            let physical_height = (height * scale).round() as i32;
            let x = anchor.center_x - physical_width / 2;
            let y = anchor.bottom_y - physical_height;

            {
                let mut expected = state
                    .overlay_expected_frame
                    .lock()
                    .map_err(|_| "Failed to lock overlay frame".to_string())?;
                *expected = Some(OverlayFrame {
                    x,
                    y,
                    width: physical_width,
                    height: physical_height,
                });
            }

            state.overlay_layout_locked.store(true, Ordering::SeqCst);

            let size_result = window
                .set_size(Size::Logical(LogicalSize::new(width, height)))
                .map_err(|e| e.to_string());

            if let Err(err) = size_result {
                state.overlay_layout_locked.store(false, Ordering::SeqCst);
                return Err(err);
            }

            let position_result = window
                .set_position(Position::Physical(PhysicalPosition::new(x, y)))
                .map_err(|e| e.to_string());

            state.overlay_layout_locked.store(false, Ordering::SeqCst);
            position_result?;
            return Ok(());
        }
    }

    window
        .set_size(Size::Logical(LogicalSize::new(width, height)))
        .map_err(|e| e.to_string())?;

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
    let bytes = include_bytes!("../icons/32x32.png");
    tauri::image::Image::from_bytes(bytes)
        .expect("tray icon 32x32.png")
        .to_owned()
}

fn reveal_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        debug_log_line("window", "reveal_main_window");
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        let _ = window.emit("overlay-revealed", ());
    }
}

fn reveal_overlay_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        debug_log_line("window", "reveal_overlay_window");
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.emit("overlay-revealed", ());
    }
}

fn hide_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        debug_log_line("window", "hide_main_window");
        let _ = window.hide();
    }
}

fn launched_from_autostart() -> bool {
    std::env::args_os().any(|arg| arg == "--autostart")
}

fn setup_tray(app: &tauri::App) -> Result<()> {
    let show_i = MenuItem::with_id(app, "show", "Show / Hide", true, None::<&str>)
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
        .tooltip("OpenWhisper – hold Ctrl+Space to talk")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    if w.is_visible().unwrap_or(false) {
                        hide_main_window(app);
                    } else {
                        reveal_main_window(app);
                    }
                }
            }
            "record" => {
                #[cfg(target_os = "windows")]
                text_inserter::save_foreground();
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
                if let Some(w) = app.get_webview_window("main") {
                    if w.is_visible().unwrap_or(false) {
                        hide_main_window(&app);
                    } else {
                        reveal_main_window(&app);
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
        overlay_anchor: std::sync::Mutex::new(None),
        overlay_layout_locked: AtomicBool::new(false),
        overlay_expected_frame: std::sync::Mutex::new(None),
    });

    tauri::Builder::default()
        .manage(app_state)
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .args(["--autostart"])
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }

            match event {
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    hide_main_window(&window.app_handle());
                }
                WindowEvent::Moved(position) => {
                    let app_state = window.state::<Arc<AppState>>().inner().clone();

                    if app_state
                        .overlay_layout_locked
                        .load(Ordering::SeqCst)
                    {
                        return;
                    }

                    if let Ok(size) = window.outer_size() {
                        if let Ok(mut expected) = app_state.overlay_expected_frame.lock()
                        {
                            if expected
                                .as_ref()
                                .is_some_and(|frame| {
                                    frame.matches(
                                        position.x,
                                        position.y,
                                        size.width as i32,
                                        size.height as i32,
                                    )
                                })
                            {
                                *expected = None;
                                return;
                            }
                        }

                        if let Ok(mut anchor) = app_state.overlay_anchor.lock() {
                            *anchor = Some(OverlayAnchor {
                                center_x: position.x + size.width as i32 / 2,
                                bottom_y: position.y + size.height as i32,
                            });
                        }
                    }
                }
                _ => {}
            }
        })
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

            if !launched_from_autostart() {
                reveal_main_window(app.handle());
            }

            #[cfg(target_os = "windows")]
            {
                std::thread::spawn(|| text_inserter::install_keyboard_hook());

                let poll_handle = app.handle().clone();
                std::thread::spawn(move || loop {
                    text_inserter::save_foreground();
                    let should_start = text_inserter::take_press_to_talk_start();
                    let should_stop = text_inserter::take_press_to_talk_stop();
                    let debug_events = text_inserter::take_debug_events();
                    if debug_events != 0 {
                        debug_log_line(
                            "hook",
                            &format!(
                                "events={} start_pending={} stop_pending={}",
                                text_inserter::describe_debug_events(debug_events),
                                should_start,
                                should_stop
                            ),
                        );
                    }
                    if should_start {
                        debug_log_line("shortcut", "emit press-to-talk-start");
                        text_inserter::save_foreground();
                        reveal_overlay_window(&poll_handle);
                        let _ = poll_handle.emit("press-to-talk-start", ());
                    }
                    if should_stop {
                        debug_log_line("shortcut", "emit press-to-talk-stop");
                        let _ = poll_handle.emit("press-to-talk-stop", ());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_get_settings,
            app_get_openai_api_key,
            app_save_settings,
            worker_ping,
            worker_list_models,
            worker_start_session,
            worker_stop_session,
            worker_poll_session_events,
            worker_append_audio_chunk,
            worker_finalize_session_audio,
            debug_log,
            open_api_keys_page,
            paste_to_target,
            overlay_apply_layout
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
