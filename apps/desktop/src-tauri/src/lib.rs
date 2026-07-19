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
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize, Size, State, Window,
    WindowEvent,
};
use tauri_plugin_opener::OpenerExt;
use tokio::sync::Mutex;
use worker_client::WorkerClient;

const SECONDARY_MONITOR_VERTICAL_OFFSET_LOGICAL: f64 = 2.0;

fn overlay_bottom_anchor(monitor_bottom: i32, work_bottom: i32, is_primary: bool, dpi: u32) -> i32 {
    const BOTTOM_MARGIN: i32 = 10;
    let vertical_offset = if is_primary {
        0
    } else {
        (SECONDARY_MONITOR_VERTICAL_OFFSET_LOGICAL * dpi as f64 / 96.0).round() as i32
    };
    work_bottom
        .saturating_sub(BOTTOM_MARGIN)
        .saturating_add(vertical_offset)
        .min(monitor_bottom.saturating_sub(BOTTOM_MARGIN))
}

// ---------------------------------------------------------------------------
// Platform: text insertion via clipboard + simulated Ctrl+V
// ---------------------------------------------------------------------------
#[cfg(target_os = "windows")]
mod text_inserter {
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};

    static TARGET_HWND: AtomicIsize = AtomicIsize::new(0);
    static TARGET_FOCUS_HWND: AtomicIsize = AtomicIsize::new(0);
    static HOOK_HANDLE: AtomicIsize = AtomicIsize::new(0);
    static CTRL_HELD: AtomicBool = AtomicBool::new(false);
    static PRESS_TO_TALK_ACTIVE: AtomicBool = AtomicBool::new(false);
    static SPACE_SWALLOWED: AtomicBool = AtomicBool::new(false);
    static PRESS_TO_TALK_START: AtomicBool = AtomicBool::new(false);
    static PRESS_TO_TALK_STOP: AtomicBool = AtomicBool::new(false);
    static DEBUG_EVENT_BITS: AtomicU32 = AtomicU32::new(0);

    const INPUT_KEYBOARD: u32 = 1;
    const KEYEVENTF_KEYUP: u32 = 0x0002;
    const VK_BACK: u16 = 0x08;
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
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const MAX_PATH: usize = 260;
    const DBG_CTRL_DOWN: u32 = 1 << 0;
    const DBG_CTRL_UP: u32 = 1 << 1;
    const DBG_SPACE_DOWN: u32 = 1 << 2;
    const DBG_SPACE_REPEAT: u32 = 1 << 3;
    const DBG_SPACE_UP: u32 = 1 << 4;
    const DBG_START_FLAG: u32 = 1 << 5;
    const DBG_STOP_BY_CTRL: u32 = 1 << 6;
    const DBG_STOP_BY_SPACE: u32 = 1 << 7;

    #[repr(C)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    struct GuiThreadInfo {
        cb_size: u32,
        flags: u32,
        hwnd_active: isize,
        hwnd_focus: isize,
        hwnd_capture: isize,
        hwnd_menu_owner: isize,
        hwnd_move_size: isize,
        hwnd_caret: isize,
        rc_caret: Rect,
    }

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
        fn SetFocus(h_wnd: isize) -> isize;
        fn IsWindow(h_wnd: isize) -> i32;
        fn GetGUIThreadInfo(id_thread: u32, pgui: *mut GuiThreadInfo) -> i32;
        fn AttachThreadInput(id_attach: u32, id_attach_to: u32, attach: i32) -> i32;
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

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThreadId() -> u32;
        fn OpenProcess(dw_desired_access: u32, b_inherit_handle: i32, dw_process_id: u32) -> isize;
        fn CloseHandle(h_object: isize) -> i32;
        fn QueryFullProcessImageNameW(
            h_process: isize,
            dw_flags: u32,
            lp_exe_name: *mut u16,
            lpdw_size: *mut u32,
        ) -> i32;
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
            let thread_id = unsafe { GetWindowThreadProcessId(hwnd, std::ptr::null_mut()) };
            let mut info = GuiThreadInfo {
                cb_size: std::mem::size_of::<GuiThreadInfo>() as u32,
                flags: 0,
                hwnd_active: 0,
                hwnd_focus: 0,
                hwnd_capture: 0,
                hwnd_menu_owner: 0,
                hwnd_move_size: 0,
                hwnd_caret: 0,
                rc_caret: Rect {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                },
            };
            if thread_id != 0 && unsafe { GetGUIThreadInfo(thread_id, &mut info) } != 0 {
                TARGET_FOCUS_HWND.store(info.hwnd_focus, Ordering::Relaxed);
            }
        }
    }

    /// Returns the executable name (without `.exe`) of the saved foreground window.
    pub fn foreground_app_name() -> Option<String> {
        let hwnd = TARGET_HWND.load(Ordering::Relaxed);
        if hwnd == 0 {
            return None;
        }
        let mut pid: u32 = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == 0 {
            return None;
        }
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle == 0 {
            return None;
        }
        let mut buf = [0u16; MAX_PATH];
        let mut size = MAX_PATH as u32;
        let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size) };
        unsafe { CloseHandle(handle) };
        if ok == 0 || size == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..size as usize]);
        path.rsplit('\\')
            .next()
            .map(|f| f.strip_suffix(".exe").unwrap_or(f).to_string())
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

    fn restore_target_focus() {
        let hwnd = TARGET_HWND.load(Ordering::Relaxed);
        if hwnd != 0 {
            unsafe { SetForegroundWindow(hwnd) };
            let focus_hwnd = TARGET_FOCUS_HWND.load(Ordering::Relaxed);
            if focus_hwnd != 0 && unsafe { IsWindow(focus_hwnd) } != 0 {
                let target_thread = unsafe { GetWindowThreadProcessId(hwnd, std::ptr::null_mut()) };
                let current_thread = unsafe { GetCurrentThreadId() };
                let attached = target_thread != 0
                    && target_thread != current_thread
                    && unsafe { AttachThreadInput(current_thread, target_thread, 1) } != 0;
                unsafe { SetFocus(focus_hwnd) };
                if attached {
                    unsafe { AttachThreadInput(current_thread, target_thread, 0) };
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
    }

    fn paste_shortcut() {
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

    pub fn restore_and_paste() {
        restore_target_focus();
        paste_shortcut();
    }

    pub fn restore_and_replace(streamed_chars: usize) {
        restore_target_focus();

        // The caret remains immediately after the chunks pasted during this
        // recording. Remove that exact draft before inserting a refined final.
        let mut remaining = streamed_chars;
        while remaining > 0 {
            let chunk_size = remaining.min(128);
            let mut inputs = Vec::with_capacity(chunk_size * 2);
            for _ in 0..chunk_size {
                inputs.push(kbd(VK_BACK, 0));
                inputs.push(kbd(VK_BACK, KEYEVENTF_KEYUP));
            }
            unsafe {
                SendInput(
                    inputs.len() as u32,
                    inputs.as_ptr(),
                    std::mem::size_of::<KbdInput>() as i32,
                );
            }
            remaining -= chunk_size;
        }
        paste_shortcut();
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
                        // Check repeat first: if we're already in press-to-talk, subsequent downs are key repeat
                        if SPACE_SWALLOWED.load(Ordering::SeqCst) && down {
                            DEBUG_EVENT_BITS.fetch_or(DBG_SPACE_REPEAT, Ordering::SeqCst);
                            return 1;
                        }

                        if down && CTRL_HELD.load(Ordering::SeqCst) {
                            SPACE_SWALLOWED.store(true, Ordering::SeqCst);
                            DEBUG_EVENT_BITS.fetch_or(DBG_SPACE_DOWN, Ordering::SeqCst);
                            if !PRESS_TO_TALK_ACTIVE.swap(true, Ordering::SeqCst) {
                                PRESS_TO_TALK_START.store(true, Ordering::SeqCst);
                                DEBUG_EVENT_BITS.fetch_or(DBG_START_FLAG, Ordering::SeqCst);
                            }
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
    use super::{PhysicalPosition, PhysicalSize, PlacementVisibility};
    use std::mem::size_of;

    const MONITOR_DEFAULTTOPRIMARY: u32 = 1;
    const MONITOR_DEFAULTTONULL: u32 = 0;
    const MDT_EFFECTIVE_DPI: u32 = 0;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOREDRAW: u32 = 0x0008;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_SHOWWINDOW: u32 = 0x0040;
    const SWP_HIDEWINDOW: u32 = 0x0080;
    const SWP_NOCOPYBITS: u32 = 0x0100;
    const SWP_NOOWNERZORDER: u32 = 0x0200;
    const SWP_DEFERERASE: u32 = 0x2000;

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
        fn GetCursorPos(lp_point: *mut Point) -> i32;
        fn GetDpiForWindow(hwnd: isize) -> u32;
        fn GetWindowRect(hwnd: isize, rect: *mut Rect) -> i32;
        fn SetWindowPos(
            hwnd: isize,
            insert_after: isize,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> i32;
    }

    #[link(name = "shcore")]
    extern "system" {
        fn GetDpiForMonitor(
            h_monitor: isize,
            dpi_type: u32,
            dpi_x: *mut u32,
            dpi_y: *mut u32,
        ) -> i32;
    }

    fn monitor_anchor_at(point: Point, fallback: u32) -> Option<(isize, i32, i32, u32)> {
        let monitor = unsafe { MonitorFromPoint(point, fallback) };
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

        let mut dpi_x = 96;
        let mut dpi_y = 96;
        if unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) } != 0 {
            dpi_x = 96;
        }

        Some((
            monitor,
            (info.rc_work.left + info.rc_work.right) / 2,
            super::overlay_bottom_anchor(
                info.rc_monitor.bottom,
                info.rc_work.bottom,
                info.dw_flags & 1 != 0,
                dpi_x,
            ),
            dpi_x,
        ))
    }

    pub fn primary_monitor_anchor() -> Option<(isize, i32, i32, u32)> {
        monitor_anchor_at(Point { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY)
    }

    pub fn cursor_monitor_anchor() -> Option<(isize, i32, i32, u32)> {
        let mut cursor = Point { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut cursor) } == 0 {
            return None;
        }
        // A cursor crossing a physical gap between displays belongs to no
        // monitor yet. Keep the committed placement until it is truly on one.
        monitor_anchor_at(cursor, MONITOR_DEFAULTTONULL)
    }

    pub fn set_window_placement(
        hwnd: isize,
        position: PhysicalPosition<i32>,
        size: PhysicalSize<u32>,
        visibility: PlacementVisibility,
    ) -> Result<(), String> {
        let visibility_flags = match visibility {
            PlacementVisibility::Keep => 0,
            PlacementVisibility::HideWithoutRedraw => {
                SWP_HIDEWINDOW | SWP_NOREDRAW | SWP_NOCOPYBITS | SWP_DEFERERASE
            }
            PlacementVisibility::Show => SWP_SHOWWINDOW,
        };
        let flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER | visibility_flags;
        let succeeded = unsafe {
            SetWindowPos(
                hwnd,
                0,
                position.x,
                position.y,
                size.width as i32,
                size.height as i32,
                flags,
            )
        };
        if succeeded == 0 {
            Err("SetWindowPos failed while committing overlay placement".to_string())
        } else {
            Ok(())
        }
    }

    pub fn window_placement(
        hwnd: isize,
    ) -> Option<(PhysicalPosition<i32>, PhysicalSize<u32>, u32)> {
        let mut rect = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
            return None;
        }
        Some((
            PhysicalPosition::new(rect.left, rect.top),
            PhysicalSize::new(
                rect.right.saturating_sub(rect.left) as u32,
                rect.bottom.saturating_sub(rect.top) as u32,
            ),
            unsafe { GetDpiForWindow(hwnd) },
        ))
    }
}

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

fn default_transcription_prompt() -> String {
    "I was editing @README.md and checking @package.json in my IDE. Let me also look at @tsconfig.json and the @src directory.".to_string()
}

fn default_transcription_model() -> String {
    "gpt-4o-mini-transcribe".to_string()
}

fn is_supported_transcription_model(model: &str) -> bool {
    matches!(
        model,
        "gpt-4o-mini-transcribe" | "gpt-4o-transcribe" | "gpt-realtime-transcribe"
    )
}

fn default_refine_enabled() -> bool {
    true
}

fn default_show_idle_pill() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AppSettings {
    #[serde(default)]
    openai_api_key: String,
    #[serde(default = "default_transcription_model")]
    transcription_model: String,
    #[serde(default = "default_transcription_prompt")]
    transcription_prompt: String,
    #[serde(default = "default_refine_enabled")]
    refine_enabled: bool,
    #[serde(default)]
    refine_prompt: String,
    #[serde(default = "default_show_idle_pill")]
    show_idle_pill: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            openai_api_key: String::new(),
            transcription_model: default_transcription_model(),
            transcription_prompt: default_transcription_prompt(),
            refine_enabled: true,
            refine_prompt: String::new(),
            show_idle_pill: true,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FrontendSettings {
    has_openai_api_key: bool,
    openai_api_key_preview: Option<String>,
    transcription_model: String,
    transcription_prompt: String,
    refine_enabled: bool,
    refine_prompt: String,
    show_idle_pill: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OverlayAnchor {
    monitor_id: isize,
    center_x: i32,
    bottom_y: i32,
    dpi: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct OverlayLayoutSpec {
    width: f64,
    height: f64,
    anchor_offset_y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OverlayPlacement {
    position: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
    dpi: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlacementVisibility {
    Keep,
    HideWithoutRedraw,
    Show,
}

const MONITOR_SWITCH_STABLE_SAMPLES: u8 = 5;

#[derive(Default)]
struct MonitorTransitionDebouncer {
    candidate: Option<OverlayAnchor>,
    samples: u8,
}

impl MonitorTransitionDebouncer {
    fn observe(
        &mut self,
        committed: Option<OverlayAnchor>,
        observed: OverlayAnchor,
    ) -> Option<OverlayAnchor> {
        if committed == Some(observed) {
            self.reset();
            return None;
        }
        if self.candidate == Some(observed) {
            self.samples = self.samples.saturating_add(1);
        } else {
            self.candidate = Some(observed);
            self.samples = 1;
        }
        (self.samples >= MONITOR_SWITCH_STABLE_SAMPLES).then_some(observed)
    }

    fn reset(&mut self) {
        self.candidate = None;
        self.samples = 0;
    }
}

struct AppState {
    worker: Mutex<Option<WorkerClient>>,
    settings: std::sync::Mutex<AppSettings>,
    overlay_anchor: std::sync::Mutex<Option<OverlayAnchor>>,
    overlay_layout: std::sync::Mutex<Option<OverlayLayoutSpec>>,
    overlay_layout_locked: AtomicBool,
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

fn overlay_window_position_for_anchor(
    anchor: OverlayAnchor,
    physical_width: i32,
    physical_height: i32,
) -> PhysicalPosition<i32> {
    let x = anchor.center_x.saturating_sub(physical_width / 2);
    let y = anchor.bottom_y.saturating_sub(physical_height);

    PhysicalPosition::new(x, y)
}

fn overlay_window_position_for_anchor_offset(
    anchor: OverlayAnchor,
    physical_width: i32,
    physical_height: i32,
    physical_bottom_offset: i32,
) -> PhysicalPosition<i32> {
    let mut position = overlay_window_position_for_anchor(anchor, physical_width, physical_height);
    position.y = position.y.saturating_add(physical_bottom_offset);
    position
}

fn physical_dimension(logical: f64, dpi: u32) -> u32 {
    (logical * dpi as f64 / 96.0).round().max(1.0) as u32
}

fn overlay_placement(anchor: OverlayAnchor, layout: OverlayLayoutSpec) -> OverlayPlacement {
    let size = PhysicalSize::new(
        physical_dimension(layout.width, anchor.dpi),
        physical_dimension(layout.height, anchor.dpi),
    );
    let offset = (layout.anchor_offset_y * anchor.dpi as f64 / 96.0).round() as i32;
    OverlayPlacement {
        position: overlay_window_position_for_anchor_offset(
            anchor,
            size.width as i32,
            size.height as i32,
            offset,
        ),
        size,
        dpi: anchor.dpi,
    }
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

fn frontend_settings_from(settings: &AppSettings) -> FrontendSettings {
    let openai_api_key_preview = masked_api_key_preview(&settings.openai_api_key);
    FrontendSettings {
        has_openai_api_key: openai_api_key_preview.is_some(),
        openai_api_key_preview,
        transcription_model: settings.transcription_model.clone(),
        transcription_prompt: settings.transcription_prompt.clone(),
        refine_enabled: settings.refine_enabled,
        refine_prompt: settings.refine_prompt.clone(),
        show_idle_pill: settings.show_idle_pill,
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
        let (openai_api_key, transcription_model) = {
            let settings = state
                .settings
                .lock()
                .map_err(|_| "Failed to lock settings".to_string())?;
            let key = if settings.openai_api_key.trim().is_empty() {
                None
            } else {
                Some(settings.openai_api_key.clone())
            };
            (key, settings.transcription_model.clone())
        };
        let worker = WorkerClient::spawn(app_handle, openai_api_key, transcription_model)
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
    openai_api_key: Option<String>,
    transcription_model: Option<String>,
    transcription_prompt: Option<String>,
    refine_enabled: Option<bool>,
    refine_prompt: Option<String>,
    show_idle_pill: Option<bool>,
) -> Result<FrontendSettings, String> {
    let updated = {
        let mut current = state
            .settings
            .lock()
            .map_err(|_| "Failed to lock settings".to_string())?
            .clone();
        if let Some(key) = openai_api_key {
            current.openai_api_key = key.trim().to_string();
        }
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
        current
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
    let (prompt, refine_enabled, refine_prompt) = {
        let s = state
            .settings
            .lock()
            .map_err(|_| "Failed to lock settings".to_string())?;
        (
            s.transcription_prompt.clone(),
            s.refine_enabled,
            s.refine_prompt.clone(),
        )
    };

    let response = with_worker_request(
        &app_handle,
        &state,
        "start_session",
        json!({
            "profile_id": profile_id,
            "prompt": prompt,
            "refine_enabled": refine_enabled,
            "refine_prompt": refine_prompt,
        }),
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
async fn paste_to_target(text: String) -> Result<(), String> {
    arboard::Clipboard::new()
        .and_then(|mut c| c.set_text(&text))
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "windows")]
    text_inserter::restore_and_paste();

    Ok(())
}

#[tauri::command]
fn capture_paste_target() {
    #[cfg(target_os = "windows")]
    text_inserter::save_foreground();
}

#[tauri::command]
async fn replace_streamed_target(streamed_text: String, final_text: String) -> Result<(), String> {
    arboard::Clipboard::new()
        .and_then(|mut c| c.set_text(&final_text))
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "windows")]
    text_inserter::restore_and_replace(streamed_text.chars().count());

    Ok(())
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OverlayRevealIntent {
    Passive,
    Interactive,
}

impl OverlayRevealIntent {
    fn should_focus(self) -> bool {
        matches!(self, Self::Interactive)
    }
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
        .tooltip("OpenWhisper – hold Ctrl+Space to talk")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => reveal_main_window(app),
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

            // The frontend's first layout commit performs the initial reveal.
            // Until then the window has no authoritative logical size or DPI
            // placement and must remain hidden.

            #[cfg(target_os = "windows")]
            {
                std::thread::spawn(|| text_inserter::install_keyboard_hook());

                let poll_handle = app.handle().clone();
                std::thread::spawn(move || {
                    let mut last_hook_log = std::time::Instant::now();
                    let mut last_hook_event = String::new();
                    let mut monitor_transition = MonitorTransitionDebouncer::default();
                    loop {
                        text_inserter::save_foreground();
                        if let Some(cursor_anchor) = cursor_overlay_anchor() {
                            let current_anchor = poll_handle
                                .state::<Arc<AppState>>()
                                .overlay_anchor
                                .lock()
                                .ok()
                                .and_then(|guard| *guard);

                            // Require five consecutive 20 ms observations before
                            // gathering and committing the destination geometry.
                            if let Some(target) =
                                monitor_transition.observe(current_anchor, cursor_anchor)
                            {
                                match sync_overlay_window_to_anchor(&poll_handle, target) {
                                    Ok(true) => monitor_transition.reset(),
                                    Ok(false) => {}
                                    Err(err) => debug_log_line(
                                        "window",
                                        &format!("monitor switch failed: {err}"),
                                    ),
                                }
                            }
                        }
                        let should_start = text_inserter::take_press_to_talk_start();
                        let should_stop = text_inserter::take_press_to_talk_stop();
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
                                        "events={} start_pending={} stop_pending={}",
                                        desc, should_start, should_stop
                                    ),
                                );
                                last_hook_log = now;
                                last_hook_event = desc;
                            }
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
            worker_list_models,
            worker_start_session,
            worker_stop_session,
            worker_poll_session_events,
            worker_append_audio_chunk,
            worker_finalize_session_audio,
            debug_log,
            get_foreground_app,
            open_api_keys_page,
            capture_paste_target,
            paste_to_target,
            replace_streamed_target,
            overlay_apply_layout
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_anchor(monitor_id: isize, center_x: i32, bottom_y: i32, dpi: u32) -> OverlayAnchor {
        OverlayAnchor {
            monitor_id,
            center_x,
            bottom_y,
            dpi,
        }
    }

    #[test]
    fn overlay_geometry_keeps_bottom_center_anchor_on_negative_monitor() {
        let anchor = test_anchor(1, -1280, 1430, 96);

        let collapsed = overlay_window_position_for_anchor(anchor, 100, 28);
        let expanded = overlay_window_position_for_anchor(anchor, 360, 520);

        assert_eq!(collapsed, PhysicalPosition::new(-1330, 1402));
        assert_eq!(expanded, PhysicalPosition::new(-1460, 910));
        assert_eq!(collapsed.x + 50, anchor.center_x);
        assert_eq!(expanded.x + 180, anchor.center_x);
        assert_eq!(collapsed.y + 28, anchor.bottom_y);
        assert_eq!(expanded.y + 520, anchor.bottom_y);
    }

    #[test]
    fn overlay_geometry_handles_monitor_above_primary() {
        let anchor = test_anchor(1, 960, -10, 96);
        assert_eq!(
            overlay_window_position_for_anchor(anchor, 188, 72),
            PhysicalPosition::new(866, -82)
        );
    }

    #[test]
    fn transcription_model_allowlist_matches_settings_ui() {
        assert!(is_supported_transcription_model("gpt-4o-mini-transcribe"));
        assert!(is_supported_transcription_model("gpt-4o-transcribe"));
        assert!(is_supported_transcription_model("gpt-realtime-transcribe"));
        assert!(!is_supported_transcription_model("gpt-5.4"));
    }

    #[test]
    fn settings_without_idle_pill_preference_keep_showing_it() {
        let settings: AppSettings = serde_json::from_value(json!({})).unwrap();

        assert!(settings.show_idle_pill);
        assert!(frontend_settings_from(&settings).show_idle_pill);
    }

    #[test]
    fn destination_dpi_alone_determines_physical_size_and_position() {
        let layout = OverlayLayoutSpec {
            width: 92.0,
            height: 28.0,
            anchor_offset_y: 2.0,
        };
        let at_125 = overlay_placement(test_anchor(1, 1000, 900, 120), layout);
        let at_200 = overlay_placement(test_anchor(2, 2000, 1700, 192), layout);

        assert_eq!(at_125.size, PhysicalSize::new(115, 35));
        assert_eq!(at_125.position, PhysicalPosition::new(943, 868));
        assert_eq!(at_200.size, PhysicalSize::new(184, 56));
        assert_eq!(at_200.position, PhysicalPosition::new(1908, 1648));
    }

    #[test]
    fn expanded_layout_grows_around_compact_anchor() {
        let anchor = test_anchor(1, 960, 1070, 96);
        let compact = overlay_window_position_for_anchor(anchor, 38, 14);
        let expanded = overlay_window_position_for_anchor_offset(anchor, 92, 26, 6);

        assert_eq!(compact.x + 19, expanded.x + 46);
        assert_eq!(compact.y + 7, expanded.y + 13);
    }

    #[test]
    fn secondary_monitor_anchor_applies_configured_offset() {
        assert_eq!(overlay_bottom_anchor(1800, 1704, true, 192), 1694);
        assert_eq!(overlay_bottom_anchor(0, -60, false, 120), -67);
        assert_eq!(overlay_bottom_anchor(1080, 1080, false, 144), 1070);
    }

    #[test]
    fn passive_overlay_reveals_never_take_keyboard_focus() {
        assert!(!OverlayRevealIntent::Passive.should_focus());
        assert!(OverlayRevealIntent::Interactive.should_focus());
    }

    #[test]
    fn repeated_entries_from_every_direction_commit_once_to_one_fixed_placement() {
        let center = test_anchor(1, 960, 1070, 96);
        let destinations = [
            test_anchor(2, -960, 1070, 120), // enter from the left
            test_anchor(3, 2880, 1430, 144), // enter from the right
            test_anchor(4, 960, -10, 192),   // enter from above
            test_anchor(5, 960, 3230, 168),  // enter from below
        ];
        let layout = OverlayLayoutSpec {
            width: 92.0,
            height: 28.0,
            anchor_offset_y: 2.0,
        };

        for destination in destinations {
            let expected = overlay_placement(destination, layout);
            let mut committed = Some(center);
            let mut transition = MonitorTransitionDebouncer::default();
            let mut destination_commits = Vec::new();

            for _ in 0..50 {
                // Approach noise from either side of the monitor boundary must
                // not move the already-visible pill.
                for observed in [destination, center, destination, center] {
                    assert_eq!(transition.observe(committed, observed), None);
                }

                let mut commits_this_entry = 0;
                for _ in 0..MONITOR_SWITCH_STABLE_SAMPLES {
                    if let Some(target) = transition.observe(committed, destination) {
                        commits_this_entry += 1;
                        destination_commits.push(overlay_placement(target, layout));
                        committed = Some(target);
                        transition.reset();
                    }
                }
                for _ in 0..20 {
                    assert_eq!(transition.observe(committed, destination), None);
                }
                assert_eq!(commits_this_entry, 1);

                for _ in 0..MONITOR_SWITCH_STABLE_SAMPLES {
                    if let Some(target) = transition.observe(committed, center) {
                        committed = Some(target);
                        transition.reset();
                    }
                }
                assert_eq!(committed, Some(center));
            }

            assert_eq!(destination_commits.len(), 50);
            assert!(destination_commits
                .iter()
                .all(|placement| *placement == expected));
        }
    }
}
