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
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, Position, Size, State, Window,
    WindowEvent,
};
use tauri_plugin_opener::OpenerExt;
use tokio::sync::Mutex;
use worker_client::WorkerClient;

const SECONDARY_MONITOR_VERTICAL_OFFSET_LOGICAL: f64 = 2.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ScreenBounds {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

fn bounds_match_monitor(window: ScreenBounds, monitor: ScreenBounds, tolerance: i32) -> bool {
    let edge_matches = |actual: i32, expected: i32| {
        (i64::from(actual) - i64::from(expected)).abs() <= i64::from(tolerance)
    };

    window.right > window.left
        && window.bottom > window.top
        && edge_matches(window.left, monitor.left)
        && edge_matches(window.top, monitor.top)
        && edge_matches(window.right, monitor.right)
        && edge_matches(window.bottom, monitor.bottom)
}

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
    use std::mem::size_of;

    const MONITOR_DEFAULTTOPRIMARY: u32 = 1;
    const MDT_EFFECTIVE_DPI: u32 = 0;

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

    fn monitor_anchor_at(point: Point) -> Option<(i32, i32, u32)> {
        let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTOPRIMARY) };
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

    pub fn primary_monitor_anchor() -> Option<(i32, i32, u32)> {
        monitor_anchor_at(Point { x: 0, y: 0 })
    }

    pub fn cursor_monitor_anchor() -> Option<(i32, i32, u32)> {
        let mut cursor = Point { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut cursor) } == 0 {
            return None;
        }
        monitor_anchor_at(cursor)
    }
}

#[cfg(target_os = "windows")]
mod fullscreen_guard {
    use super::{bounds_match_monitor, ScreenBounds};
    use std::ffi::c_void;
    use std::mem::size_of;

    const MONITOR_DEFAULTTONEAREST: u32 = 2;
    const GWL_STYLE: i32 = -16;
    const WS_CAPTION: u32 = 0x00C0_0000;
    const SW_SHOWMAXIMIZED: u32 = 3;
    const DWMWA_EXTENDED_FRAME_BOUNDS: u32 = 9;
    const DWMWA_CLOAKED: u32 = 14;
    const FULLSCREEN_EDGE_TOLERANCE: i32 = 2;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    impl From<Rect> for ScreenBounds {
        fn from(value: Rect) -> Self {
            Self {
                left: value.left,
                top: value.top,
                right: value.right,
                bottom: value.bottom,
            }
        }
    }

    #[repr(C)]
    struct MonitorInfo {
        cb_size: u32,
        rc_monitor: Rect,
        rc_work: Rect,
        dw_flags: u32,
    }

    #[repr(C)]
    struct WindowPlacement {
        length: u32,
        flags: u32,
        show_cmd: u32,
        min_position: Point,
        max_position: Point,
        normal_position: Rect,
    }

    struct EnumContext {
        target_monitor: isize,
        monitor_bounds: ScreenBounds,
        own_process_id: u32,
        found: bool,
    }

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(
            callback: Option<unsafe extern "system" fn(isize, isize) -> i32>,
            l_param: isize,
        ) -> i32;
        fn GetClassNameW(h_wnd: isize, class_name: *mut u16, max_count: i32) -> i32;
        fn GetCursorPos(point: *mut Point) -> i32;
        fn GetMonitorInfoW(h_monitor: isize, monitor_info: *mut MonitorInfo) -> i32;
        fn GetWindowLongW(h_wnd: isize, index: i32) -> i32;
        fn GetWindowPlacement(h_wnd: isize, placement: *mut WindowPlacement) -> i32;
        fn GetWindowRect(h_wnd: isize, rect: *mut Rect) -> i32;
        fn GetWindowThreadProcessId(h_wnd: isize, process_id: *mut u32) -> u32;
        fn IsIconic(h_wnd: isize) -> i32;
        fn IsWindowVisible(h_wnd: isize) -> i32;
        fn MonitorFromPoint(point: Point, flags: u32) -> isize;
        fn MonitorFromWindow(h_wnd: isize, flags: u32) -> isize;
    }

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmGetWindowAttribute(
            h_wnd: isize,
            attribute: u32,
            value: *mut c_void,
            value_size: u32,
        ) -> i32;
    }

    fn is_shell_surface(h_wnd: isize) -> bool {
        let mut name = [0u16; 64];
        let length = unsafe { GetClassNameW(h_wnd, name.as_mut_ptr(), name.len() as i32) };
        if length <= 0 {
            return false;
        }

        matches!(
            String::from_utf16_lossy(&name[..length as usize]).as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        )
    }

    fn visible_window_bounds(h_wnd: isize) -> Option<ScreenBounds> {
        let mut cloaked = 0u32;
        let cloak_result = unsafe {
            DwmGetWindowAttribute(
                h_wnd,
                DWMWA_CLOAKED,
                (&mut cloaked as *mut u32).cast(),
                size_of::<u32>() as u32,
            )
        };
        if cloak_result == 0 && cloaked != 0 {
            return None;
        }

        let mut rect = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let dwm_result = unsafe {
            DwmGetWindowAttribute(
                h_wnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                (&mut rect as *mut Rect).cast(),
                size_of::<Rect>() as u32,
            )
        };
        if dwm_result != 0 && unsafe { GetWindowRect(h_wnd, &mut rect) } == 0 {
            return None;
        }

        Some(rect.into())
    }

    fn is_regular_maximized_window(h_wnd: isize) -> bool {
        let mut placement = WindowPlacement {
            length: size_of::<WindowPlacement>() as u32,
            flags: 0,
            show_cmd: 0,
            min_position: Point { x: 0, y: 0 },
            max_position: Point { x: 0, y: 0 },
            normal_position: Rect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
        };
        let has_placement = unsafe { GetWindowPlacement(h_wnd, &mut placement) } != 0;
        let style = unsafe { GetWindowLongW(h_wnd, GWL_STYLE) } as u32;

        has_placement && placement.show_cmd == SW_SHOWMAXIMIZED && style & WS_CAPTION != 0
    }

    unsafe extern "system" fn inspect_window(h_wnd: isize, l_param: isize) -> i32 {
        let context = &mut *(l_param as *mut EnumContext);
        if IsWindowVisible(h_wnd) == 0 || IsIconic(h_wnd) != 0 || is_shell_surface(h_wnd) {
            return 1;
        }

        let mut process_id = 0u32;
        GetWindowThreadProcessId(h_wnd, &mut process_id);
        if process_id == 0 || process_id == context.own_process_id {
            return 1;
        }

        if MonitorFromWindow(h_wnd, MONITOR_DEFAULTTONEAREST) != context.target_monitor
            || is_regular_maximized_window(h_wnd)
        {
            return 1;
        }

        if visible_window_bounds(h_wnd).is_some_and(|bounds| {
            bounds_match_monitor(bounds, context.monitor_bounds, FULLSCREEN_EDGE_TOLERANCE)
        }) {
            context.found = true;
            return 0;
        }

        1
    }

    pub fn cursor_monitor_has_fullscreen_window() -> bool {
        let mut cursor = Point { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut cursor) } == 0 {
            return false;
        }

        let monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) };
        if monitor == 0 {
            return false;
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
            return false;
        }

        let mut context = EnumContext {
            target_monitor: monitor,
            monitor_bounds: info.rc_monitor.into(),
            own_process_id: std::process::id(),
            found: false,
        };
        unsafe {
            EnumWindows(
                Some(inspect_window),
                (&mut context as *mut EnumContext) as isize,
            )
        };
        context.found
    }
}

#[cfg(target_os = "windows")]
fn primary_overlay_anchor() -> Option<OverlayAnchor> {
    overlay_positioner::primary_monitor_anchor().map(|(center_x, bottom_y, dpi)| OverlayAnchor {
        center_x,
        bottom_y,
        dpi,
    })
}

#[cfg(target_os = "windows")]
fn cursor_overlay_anchor() -> Option<OverlayAnchor> {
    overlay_positioner::cursor_monitor_anchor().map(|(center_x, bottom_y, dpi)| OverlayAnchor {
        center_x,
        bottom_y,
        dpi,
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
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            openai_api_key: String::new(),
            transcription_model: default_transcription_model(),
            transcription_prompt: default_transcription_prompt(),
            refine_enabled: true,
            refine_prompt: String::new(),
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OverlayAnchor {
    center_x: i32,
    bottom_y: i32,
    dpi: u32,
}

struct AppState {
    worker: Mutex<Option<WorkerClient>>,
    settings: std::sync::Mutex<AppSettings>,
    overlay_anchor: std::sync::Mutex<Option<OverlayAnchor>>,
    overlay_anchor_offset_logical: std::sync::Mutex<f64>,
    overlay_layout_locked: AtomicBool,
    overlay_visibility_requested: AtomicBool,
    overlay_fullscreen_suppressed: AtomicBool,
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

fn scale_physical_dimension_for_monitor(current: u32, current_scale: f64, target_dpi: u32) -> i32 {
    let target_scale = target_dpi as f64 / 96.0;
    (current as f64 * target_scale / current_scale).round() as i32
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

    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().map_err(|e| e.to_string())? {
            let size = window.outer_size().map_err(|e| e.to_string())?;
            let current_scale = window.scale_factor().map_err(|e| e.to_string())?;
            let target_width =
                scale_physical_dimension_for_monitor(size.width, current_scale, anchor.dpi);
            let target_height =
                scale_physical_dimension_for_monitor(size.height, current_scale, anchor.dpi);
            let anchor_offset_logical = *state
                .overlay_anchor_offset_logical
                .lock()
                .map_err(|_| "Failed to lock overlay anchor offset".to_string())?;
            let target_offset = (anchor_offset_logical * anchor.dpi as f64 / 96.0).round() as i32;
            let position = overlay_window_position_for_anchor_offset(
                anchor,
                target_width,
                target_height,
                target_offset,
            );
            window
                .set_position(Position::Physical(position))
                .map_err(|e| e.to_string())?;
            debug_log_line(
                "window",
                &format!(
                    "monitor switch anchor=({}, {}) dpi={} position=({}, {}) predicted_size={}x{}",
                    anchor.center_x,
                    anchor.bottom_y,
                    anchor.dpi,
                    position.x,
                    position.y,
                    target_width,
                    target_height
                ),
            );
        }
    }

    let mut current = state
        .overlay_anchor
        .lock()
        .map_err(|_| "Failed to lock overlay anchor".to_string())?;
    *current = Some(anchor);
    Ok(true)
}

#[cfg(target_os = "windows")]
fn position_main_window_before_reveal(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let Some(window) = app.get_webview_window("main") else {
        return Ok(());
    };
    let Some(anchor) = resolve_overlay_anchor(&state)? else {
        return Ok(());
    };

    let size = window.outer_size().map_err(|e| e.to_string())?;
    let anchor_offset_logical = *state
        .overlay_anchor_offset_logical
        .lock()
        .map_err(|_| "Failed to lock overlay anchor offset".to_string())?;
    let physical_anchor_offset = (anchor_offset_logical * anchor.dpi as f64 / 96.0).round() as i32;
    let position = overlay_window_position_for_anchor_offset(
        anchor,
        size.width as i32,
        size.height as i32,
        physical_anchor_offset,
    );

    window
        .set_position(Position::Physical(position))
        .map_err(|e| e.to_string())
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
        let anchor = resolve_overlay_anchor(&app_state)?;
        *app_state
            .overlay_anchor_offset_logical
            .lock()
            .map_err(|_| "Failed to lock overlay anchor offset".to_string())? = anchor_offset_y;

        if let Some(anchor) = anchor {
            let before_size = window.outer_size().map_err(|e| e.to_string())?;
            let scale = window.scale_factor().map_err(|e| e.to_string())?;
            debug_log_line(
                "layout",
                &format!(
                    "request logical={}x{} anchor=({}, {}) before={}x{} scale={}",
                    width,
                    height,
                    anchor.center_x,
                    anchor.bottom_y,
                    before_size.width,
                    before_size.height,
                    scale
                ),
            );
            let size_result = window
                .set_size(Size::Logical(LogicalSize::new(width, height)))
                .map_err(|e| e.to_string());

            if let Err(err) = size_result {
                return Err(err);
            }

            // WebView2 applies native resizes asynchronously. Poll until the
            // outer frame changes and stabilizes instead of imposing a fixed
            // 50 ms pause on every hover transition. The bounded fallback still
            // protects mixed-DPI layouts where the resize takes longer.
            let mut physical_size = before_size;
            let mut changed = false;
            let mut waited_ms = 0;
            for _ in 0..10 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                waited_ms += 5;
                let observed = window.outer_size().map_err(|e| e.to_string())?;
                if observed != before_size {
                    if changed && observed == physical_size {
                        physical_size = observed;
                        break;
                    }
                    changed = true;
                }
                physical_size = observed;
            }

            let physical_anchor_offset = (anchor_offset_y * scale).round() as i32;
            let position = overlay_window_position_for_anchor_offset(
                anchor,
                physical_size.width as i32,
                physical_size.height as i32,
                physical_anchor_offset,
            );
            debug_log_line(
                "layout",
                &format!(
                    "settled={}x{} position=({}, {}) wait_ms={}",
                    physical_size.width, physical_size.height, position.x, position.y, waited_ms
                ),
            );

            window
                .set_position(Position::Physical(position))
                .map_err(|e| e.to_string())?;
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

fn show_window_now(app: &AppHandle, intent: OverlayRevealIntent) {
    if let Some(window) = app.get_webview_window("main") {
        #[cfg(target_os = "windows")]
        if let Err(err) = position_main_window_before_reveal(app) {
            debug_log_line(
                "window",
                &format!("position_main_window_before_reveal failed: {err}"),
            );
        }

        debug_log_line("window", "reveal_window");
        let _ = window.show();
        let _ = window.unminimize();
        if intent.should_focus() {
            let _ = window.set_focus();
        }
        let _ = window.emit("overlay-revealed", ());
    }
}

fn reveal_window(app: &AppHandle, intent: OverlayRevealIntent) {
    let state = app.state::<Arc<AppState>>();
    state
        .overlay_visibility_requested
        .store(true, Ordering::SeqCst);
    if state.overlay_fullscreen_suppressed.load(Ordering::SeqCst) {
        debug_log_line(
            "window",
            "reveal deferred while fullscreen content is present",
        );
        return;
    }

    show_window_now(app, intent);
}

fn reveal_main_window(app: &AppHandle) {
    reveal_window(app, OverlayRevealIntent::Interactive);
}

fn reveal_overlay_window(app: &AppHandle) {
    reveal_window(app, OverlayRevealIntent::Passive);
}

fn hide_main_window(app: &AppHandle) {
    app.state::<Arc<AppState>>()
        .overlay_visibility_requested
        .store(false, Ordering::SeqCst);
    if let Some(window) = app.get_webview_window("main") {
        debug_log_line("window", "hide_main_window");
        let _ = window.hide();
    }
}

fn overlay_visibility_requested(app: &AppHandle) -> bool {
    app.state::<Arc<AppState>>()
        .overlay_visibility_requested
        .load(Ordering::SeqCst)
}

#[cfg(target_os = "windows")]
fn set_fullscreen_suppressed(app: &AppHandle, suppressed: bool) {
    let state = app.state::<Arc<AppState>>();
    let was_suppressed = state
        .overlay_fullscreen_suppressed
        .swap(suppressed, Ordering::SeqCst);
    if was_suppressed == suppressed {
        return;
    }

    if suppressed {
        debug_log_line("window", "hide over fullscreen content");
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.hide();
        }
    } else if state.overlay_visibility_requested.load(Ordering::SeqCst) {
        debug_log_line("window", "restore after fullscreen content");
        show_window_now(app, OverlayRevealIntent::Passive);
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
                if overlay_visibility_requested(app) {
                    hide_main_window(app);
                } else {
                    reveal_main_window(app);
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
                if overlay_visibility_requested(&app) {
                    hide_main_window(&app);
                } else {
                    reveal_main_window(&app);
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
        overlay_anchor_offset_logical: std::sync::Mutex::new(0.0),
        overlay_layout_locked: AtomicBool::new(false),
        overlay_visibility_requested: AtomicBool::new(false),
        overlay_fullscreen_suppressed: AtomicBool::new(false),
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
                    hide_main_window(&window.app_handle());
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

            if !launched_from_autostart() {
                // The passive overlay must never steal focus from the field the
                // user is typing into during app startup or a dev relaunch.
                reveal_overlay_window(app.handle());
            }

            #[cfg(target_os = "windows")]
            {
                std::thread::spawn(|| text_inserter::install_keyboard_hook());

                let poll_handle = app.handle().clone();
                std::thread::spawn(move || {
                    let mut last_hook_log = std::time::Instant::now();
                    let mut last_hook_event = String::new();
                    let mut monitor_candidate: Option<OverlayAnchor> = None;
                    let mut monitor_candidate_samples = 0u8;
                    let mut next_fullscreen_check = std::time::Instant::now();
                    loop {
                        text_inserter::save_foreground();
                        if let Some(cursor_anchor) = cursor_overlay_anchor() {
                            let current_anchor = poll_handle
                                .state::<Arc<AppState>>()
                                .overlay_anchor
                                .lock()
                                .ok()
                                .and_then(|guard| *guard);

                            if current_anchor == Some(cursor_anchor) {
                                monitor_candidate = None;
                                monitor_candidate_samples = 0;
                            } else {
                                if monitor_candidate == Some(cursor_anchor) {
                                    monitor_candidate_samples =
                                        monitor_candidate_samples.saturating_add(1);
                                } else {
                                    monitor_candidate = Some(cursor_anchor);
                                    monitor_candidate_samples = 1;
                                }

                                // Require five consecutive 20 ms observations
                                // before switching. This filters pointer noise at
                                // a shared monitor edge while keeping transitions
                                // responsive once the cursor is truly on a screen.
                                if monitor_candidate_samples >= 5 {
                                    match sync_overlay_window_to_anchor(&poll_handle, cursor_anchor)
                                    {
                                        Ok(true) => {
                                            monitor_candidate = None;
                                            monitor_candidate_samples = 0;
                                        }
                                        Ok(false) => {}
                                        Err(err) => debug_log_line(
                                            "window",
                                            &format!("monitor switch failed: {err}"),
                                        ),
                                    }
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

                        let now = std::time::Instant::now();
                        if now >= next_fullscreen_check {
                            set_fullscreen_suppressed(
                                &poll_handle,
                                fullscreen_guard::cursor_monitor_has_fullscreen_window(),
                            );
                            next_fullscreen_check = now + std::time::Duration::from_millis(200);
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

    #[test]
    fn overlay_geometry_keeps_bottom_center_anchor_on_negative_monitor() {
        let anchor = OverlayAnchor {
            center_x: -1280,
            bottom_y: 1430,
            dpi: 96,
        };

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
        let anchor = OverlayAnchor {
            center_x: 960,
            bottom_y: -10,
            dpi: 96,
        };
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
    fn monitor_switch_predicts_mixed_dpi_outer_size() {
        assert_eq!(scale_physical_dimension_for_monitor(161, 1.25, 192), 258);
        assert_eq!(scale_physical_dimension_for_monitor(44, 1.25, 192), 70);
        assert_eq!(scale_physical_dimension_for_monitor(258, 2.0, 120), 161);
        assert_eq!(scale_physical_dimension_for_monitor(70, 2.0, 120), 44);
    }

    #[test]
    fn expanded_layout_grows_around_compact_anchor() {
        let anchor = OverlayAnchor {
            center_x: 960,
            bottom_y: 1070,
            dpi: 96,
        };
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
    fn fullscreen_bounds_must_match_all_monitor_edges() {
        let monitor = ScreenBounds {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };

        assert!(bounds_match_monitor(monitor, monitor, 2));
        assert!(bounds_match_monitor(
            ScreenBounds {
                left: -1,
                top: 1,
                right: 1921,
                bottom: 1079,
            },
            monitor,
            2,
        ));
        assert!(!bounds_match_monitor(
            ScreenBounds {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1040,
            },
            monitor,
            2,
        ));
        assert!(!bounds_match_monitor(
            ScreenBounds {
                left: -1920,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            monitor,
            2,
        ));
    }
}
