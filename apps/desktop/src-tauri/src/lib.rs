mod api_key_store;
// Parts of the insertion rules are used only by the Windows writer.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod paste_target;
mod worker_client;
mod worker_connection;

use anyhow::{anyhow, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
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
use uuid::Uuid;
use worker_client::WorkerClient;

const SECONDARY_MONITOR_VERTICAL_OFFSET_LOGICAL: f64 = 2.0;
const RECORDING_RETENTION_COUNT: usize = 10;

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
// Platform: writing dictated text into the field focused when recording began
// ---------------------------------------------------------------------------
//
// A background loop tracks the last focused window outside OpenWhisper, so
// that clicking the pill still knows where the user was. When a recording
// starts, `capture_target` locks that window and its focused control for the
// whole recording; the loop never changes a locked target. Before every write
// the target is checked again (same window and control, not a password field,
// no key press or click since the last write) and `paste_target` decides what
// may be written. Live pieces are typed as Unicode keystrokes. The final text
// is pasted through the clipboard, which is put back afterwards; the temporary
// text is kept out of clipboard history and cloud sync.
#[cfg(target_os = "windows")]
mod text_inserter {
    use crate::paste_target::{self, Blocked, FinalWrite, LiveWrite, Observation};
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    static TRACKED_HWND: AtomicIsize = AtomicIsize::new(0);
    static TRACKED_FOCUS_HWND: AtomicIsize = AtomicIsize::new(0);
    static HOOK_HANDLE: AtomicIsize = AtomicIsize::new(0);
    static MOUSE_HOOK_HANDLE: AtomicIsize = AtomicIsize::new(0);
    static CTRL_HELD: AtomicBool = AtomicBool::new(false);
    static HOLD_TO_TALK_ENABLED: AtomicBool = AtomicBool::new(true);
    static PRESS_TO_TALK_ACTIVE: AtomicBool = AtomicBool::new(false);
    static SPACE_SWALLOWED: AtomicBool = AtomicBool::new(false);
    static PRESS_TO_TALK_START: AtomicBool = AtomicBool::new(false);
    static PRESS_TO_TALK_STOP: AtomicBool = AtomicBool::new(false);
    static TOGGLE_RECORDING: AtomicBool = AtomicBool::new(false);
    static DEBUG_EVENT_BITS: AtomicU32 = AtomicU32::new(0);
    /// Set by the input hooks when the user presses a key or clicks outside
    /// OpenWhisper; cleared when a recording starts and after every write.
    static USER_INPUT_SINCE_WRITE: AtomicBool = AtomicBool::new(false);
    static SESSION: Mutex<Option<PasteSession>> = Mutex::new(None);

    /// The target locked for one recording and the live draft typed into it.
    struct PasteSession {
        hwnd: isize,
        focus_hwnd: isize,
        draft: String,
        live_stopped: Option<Blocked>,
    }

    /// How a write ended, for the frontend.
    pub enum WriteOutcome {
        Typed,
        Replaced,
        Pasted,
        Kept,
        Stopped(Blocked),
        Copied(Blocked),
    }

    const INPUT_KEYBOARD: u32 = 1;
    const KEYEVENTF_KEYUP: u32 = 0x0002;
    const KEYEVENTF_UNICODE: u32 = 0x0004;
    const VK_BACK: u16 = 0x08;
    const VK_CONTROL: u16 = 0x11;
    const VK_V: u16 = 0x56;
    const VK_LCONTROL: u16 = 0xA2;
    const VK_RCONTROL: u16 = 0xA3;
    const VK_LMENU: i32 = 0xA4;
    const VK_RMENU: i32 = 0xA5;
    const VK_LWIN: i32 = 0x5B;
    const VK_RWIN: i32 = 0x5C;

    const WH_KEYBOARD_LL: i32 = 13;
    const WH_MOUSE_LL: i32 = 14;
    const WM_NULL: u32 = 0x0000;
    const WM_KEYDOWN: u32 = 0x0100;
    const WM_KEYUP: u32 = 0x0101;
    const WM_SYSKEYDOWN: u32 = 0x0104;
    const WM_SYSKEYUP: u32 = 0x0105;
    const WM_LBUTTONDOWN: u32 = 0x0201;
    const WM_RBUTTONDOWN: u32 = 0x0204;
    const WM_MBUTTONDOWN: u32 = 0x0207;
    const WM_XBUTTONDOWN: u32 = 0x020B;
    const VK_SHIFT_CODE: u32 = 0x10;
    const VK_LSHIFT_CODE: u32 = 0xA0;
    const VK_RSHIFT_CODE: u32 = 0xA1;
    const VK_CONTROL_CODE: u32 = 0x11;
    const VK_LCONTROL_CODE: u32 = 0xA2;
    const VK_RCONTROL_CODE: u32 = 0xA3;
    const VK_SPACE: u32 = 0x20;
    const LLKHF_INJECTED: u32 = 0x10;
    const LLMHF_INJECTED: u32 = 0x01;
    const SMTO_ABORTIFHUNG: u32 = 0x0002;
    const GWL_STYLE: i32 = -16;
    const ES_PASSWORD: i32 = 0x0020;
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
    const DBG_TOGGLE_FLAG: u32 = 1 << 8;

    /// Ctrl+V is only queued: the target reads the clipboard when it handles
    /// the key, and browsers read it on another thread a moment later.
    const PASTE_HANDLED_TIMEOUT_MS: u32 = 1000;
    const PASTE_SETTLE: Duration = Duration::from_millis(300);
    const CLIPBOARD_OPEN_ATTEMPTS: usize = 10;

    #[repr(C)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Point {
        x: i32,
        y: i32,
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

    #[repr(C)]
    struct MsLlHookStruct {
        pt: Point,
        mouse_data: u32,
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
        fn GetAsyncKeyState(v_key: i32) -> i16;
        fn GetWindowLongW(h_wnd: isize, n_index: i32) -> i32;
        fn GetClassNameW(h_wnd: isize, lp_class_name: *mut u16, n_max_count: i32) -> i32;
        fn WindowFromPoint(point: Point) -> isize;
        fn SendMessageTimeoutW(
            h_wnd: isize,
            msg: u32,
            w_param: usize,
            l_param: isize,
            fu_flags: u32,
            u_timeout: u32,
            lpdw_result: *mut usize,
        ) -> isize;
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

    /// The control with keyboard focus inside `hwnd`'s UI thread, or 0.
    fn focused_control(hwnd: isize) -> isize {
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
            info.hwnd_focus
        } else {
            0
        }
    }

    /// Follow the current foreground window, ignoring our own process. Runs
    /// every 20 ms; it never changes the target locked for a recording.
    pub fn track_foreground() {
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd != 0 && !is_own_process(hwnd) {
            TRACKED_HWND.store(hwnd, Ordering::Relaxed);
            TRACKED_FOCUS_HWND.store(focused_control(hwnd), Ordering::Relaxed);
        }
    }

    /// Lock the window and control the user was in for this recording.
    pub fn capture_target() {
        track_foreground();
        let session = PasteSession {
            hwnd: TRACKED_HWND.load(Ordering::Relaxed),
            focus_hwnd: TRACKED_FOCUS_HWND.load(Ordering::Relaxed),
            draft: String::new(),
            live_stopped: None,
        };
        *SESSION.lock().unwrap_or_else(|e| e.into_inner()) = Some(session);
        USER_INPUT_SINCE_WRITE.store(false, Ordering::SeqCst);
    }

    /// Returns the executable name (without `.exe`) of the tracked foreground window.
    pub fn foreground_app_name() -> Option<String> {
        let hwnd = TRACKED_HWND.load(Ordering::Relaxed);
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

    /// Returns `true` once when Ctrl+Space should toggle recording.
    pub fn take_toggle_recording() -> bool {
        TOGGLE_RECORDING.swap(false, Ordering::SeqCst)
    }

    pub fn set_hold_to_talk_enabled(enabled: bool) {
        HOLD_TO_TALK_ENABLED.store(enabled, Ordering::SeqCst);
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
        if bits & DBG_TOGGLE_FLAG != 0 {
            events.push("toggle_flag");
        }
        events.join(",")
    }

    // ----- Checking the locked target before a write -----

    fn key_held(vk: i32) -> bool {
        let state = unsafe { GetAsyncKeyState(vk) };
        state < 0
    }

    fn restore_target_focus(session: &PasteSession) {
        unsafe { SetForegroundWindow(session.hwnd) };
        let focus_hwnd = session.focus_hwnd;
        if focus_hwnd != 0 && unsafe { IsWindow(focus_hwnd) } != 0 {
            let target_thread =
                unsafe { GetWindowThreadProcessId(session.hwnd, std::ptr::null_mut()) };
            let current_thread = unsafe { GetCurrentThreadId() };
            let attached = target_thread != 0
                && target_thread != current_thread
                && unsafe { AttachThreadInput(current_thread, target_thread, 1) } != 0;
            unsafe { SetFocus(focus_hwnd) };
            if attached {
                unsafe { AttachThreadInput(current_thread, target_thread, 0) };
            }
        }
        std::thread::sleep(Duration::from_millis(80));
    }

    /// The target keeps focus, or gets it back when the user only clicked
    /// OpenWhisper. Focus is never taken from another app the user moved to.
    fn bring_target_forward(session: &PasteSession) -> bool {
        let foreground = unsafe { GetForegroundWindow() };
        if foreground == session.hwnd {
            return true;
        }
        if foreground != 0 && is_own_process(foreground) {
            restore_target_focus(session);
            return unsafe { GetForegroundWindow() } == session.hwnd;
        }
        false
    }

    /// Classic Win32 edit controls mark password fields with `ES_PASSWORD`.
    fn is_win32_password_edit(hwnd: isize) -> bool {
        if hwnd == 0 || unsafe { GetWindowLongW(hwnd, GWL_STYLE) } & ES_PASSWORD == 0 {
            return false;
        }
        let mut buf = [0u16; 64];
        let len = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        if len <= 0 {
            return false;
        }
        let class = String::from_utf16_lossy(&buf[..len as usize]).to_ascii_lowercase();
        class == "edit" || class.starts_with("richedit")
    }

    fn observe(session: &PasteSession) -> Observation {
        let target_alive = session.hwnd != 0 && unsafe { IsWindow(session.hwnd) } != 0;
        let foreground_is_target = target_alive && bring_target_forward(session);
        let focus_now = if foreground_is_target {
            focused_control(session.hwnd)
        } else {
            0
        };
        let same_field = if session.focus_hwnd != 0 && focus_now != 0 {
            Some(focus_now == session.focus_hwnd)
        } else {
            None
        };
        let is_password = foreground_is_target
            && (uia::focused_element_is_password().unwrap_or(false)
                || is_win32_password_edit(focus_now));
        Observation {
            target_alive,
            foreground_is_target,
            same_field,
            is_password,
            user_input_since_write: USER_INPUT_SINCE_WRITE.load(Ordering::SeqCst),
            shortcut_modifier_held: [VK_LMENU, VK_RMENU, VK_LWIN, VK_RWIN]
                .into_iter()
                .any(key_held),
        }
    }

    fn lock_session() -> std::sync::MutexGuard<'static, Option<PasteSession>> {
        SESSION.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Type one live piece of the transcript into the locked target.
    pub fn type_live(text: &str) -> WriteOutcome {
        let mut guard = lock_session();
        let Some(session) = guard.as_mut() else {
            return WriteOutcome::Stopped(Blocked::NoTarget);
        };
        if let Some(blocked) = session.live_stopped {
            return WriteOutcome::Stopped(blocked);
        }
        let observation = observe(session);
        match paste_target::decide_live(&observation, !session.draft.is_empty()) {
            LiveWrite::Type => {
                let typed = paste_target::typeable_text(text);
                release_held_ctrl();
                type_unicode(&typed);
                session.draft.push_str(&typed);
                USER_INPUT_SINCE_WRITE.store(false, Ordering::SeqCst);
                WriteOutcome::Typed
            }
            LiveWrite::Stop(blocked) => {
                session.live_stopped = Some(blocked);
                WriteOutcome::Stopped(blocked)
            }
        }
    }

    /// Write the final transcript and end the recording's target lock.
    pub fn insert_final(text: &str) -> Result<WriteOutcome, String> {
        let Some(session) = lock_session().take() else {
            return copy_instead(text, Blocked::NoTarget);
        };
        let observation = observe(&session);
        let decision = paste_target::decide_final(
            &observation,
            &session.draft,
            session.live_stopped.is_none(),
            text,
        );
        match decision {
            FinalWrite::Replace { delete_chars } => {
                release_held_ctrl();
                press_backspace(delete_chars);
                paste_restoring_clipboard(text, session.hwnd)?;
                Ok(WriteOutcome::Replaced)
            }
            FinalWrite::Paste => {
                paste_restoring_clipboard(text, session.hwnd)?;
                Ok(WriteOutcome::Pasted)
            }
            FinalWrite::KeepDraft => Ok(WriteOutcome::Kept),
            FinalWrite::Copy(blocked) => copy_instead(text, blocked),
        }
    }

    /// Leave the transcript on the clipboard when it cannot be inserted. Text
    /// meant for a password field stays out of clipboard history.
    fn copy_instead(text: &str, blocked: Blocked) -> Result<WriteOutcome, String> {
        use arboard::SetExtWindows;
        let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        let set = clipboard.set();
        let set = if blocked == Blocked::PasswordField {
            set.exclude_from_history().exclude_from_cloud()
        } else {
            set
        };
        set.text(text).map_err(|e| e.to_string())?;
        Ok(WriteOutcome::Copied(blocked))
    }

    // ----- Keystrokes -----

    fn send_inputs(inputs: &[KbdInput]) {
        if inputs.is_empty() {
            return;
        }
        unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<KbdInput>() as i32,
            );
        }
    }

    /// With Ctrl+Space held for hold-to-talk, typed letters would become
    /// shortcuts and Backspace would delete whole words. Release Ctrl first;
    /// the keyboard hook ignores injected keys, so the gesture keeps working.
    fn release_held_ctrl() {
        let inputs: Vec<KbdInput> = [VK_LCONTROL, VK_RCONTROL]
            .into_iter()
            .filter(|vk| key_held(*vk as i32))
            .map(|vk| kbd(vk, KEYEVENTF_KEYUP))
            .collect();
        send_inputs(&inputs);
    }

    fn type_unicode(text: &str) {
        let units: Vec<u16> = text.encode_utf16().collect();
        for chunk in units.chunks(64) {
            let mut inputs = Vec::with_capacity(chunk.len() * 2);
            for &unit in chunk {
                inputs.push(unicode_key(unit, 0));
                inputs.push(unicode_key(unit, KEYEVENTF_KEYUP));
            }
            send_inputs(&inputs);
        }
    }

    fn press_backspace(count: usize) {
        let mut remaining = count;
        while remaining > 0 {
            let chunk_size = remaining.min(128);
            let mut inputs = Vec::with_capacity(chunk_size * 2);
            for _ in 0..chunk_size {
                inputs.push(kbd(VK_BACK, 0));
                inputs.push(kbd(VK_BACK, KEYEVENTF_KEYUP));
            }
            send_inputs(&inputs);
            remaining -= chunk_size;
        }
    }

    fn paste_shortcut() {
        send_inputs(&[
            kbd(VK_CONTROL, 0),
            kbd(VK_V, 0),
            kbd(VK_V, KEYEVENTF_KEYUP),
            kbd(VK_CONTROL, KEYEVENTF_KEYUP),
        ]);
    }

    // ----- Clipboard paste that puts the user's clipboard back -----

    /// Paste `text` with Ctrl+V, then restore what the user had copied,
    /// unless something else changed the clipboard in the meantime.
    fn paste_restoring_clipboard(text: &str, target: isize) -> Result<(), String> {
        let saved = {
            let _open = clipboard_win::Clipboard::new_attempts(CLIPBOARD_OPEN_ATTEMPTS)
                .map_err(|e| format!("Could not open the clipboard: {e}"))?;
            let saved = save_clipboard();
            clipboard_win::raw::set_string(text)
                .map_err(|e| format!("Could not set the clipboard: {e}"))?;
            mark_clipboard_item(&[
                "ExcludeClipboardContentFromMonitorProcessing",
                "CanIncludeInClipboardHistory",
                "CanUploadToCloudClipboard",
            ]);
            saved
        };
        let ours = clipboard_win::raw::seq_num();
        paste_shortcut();
        wait_until_pasted(target);
        if clipboard_win::raw::seq_num() == ours {
            if let Ok(_open) = clipboard_win::Clipboard::new_attempts(CLIPBOARD_OPEN_ATTEMPTS) {
                restore_clipboard(&saved);
            }
        }
        Ok(())
    }

    fn wait_until_pasted(target: isize) {
        std::thread::sleep(Duration::from_millis(50));
        let mut result = 0usize;
        unsafe {
            SendMessageTimeoutW(
                target,
                WM_NULL,
                0,
                0,
                SMTO_ABORTIFHUNG,
                PASTE_HANDLED_TIMEOUT_MS,
                &mut result,
            )
        };
        std::thread::sleep(PASTE_SETTLE);
    }

    /// Clipboard formats whose data is plain memory. Bitmaps, metafiles and
    /// palettes are GDI handles (their DIB forms are kept), and private
    /// formats hold handles only their owner understands.
    fn is_memory_format(format: u32) -> bool {
        const CF_BITMAP: u32 = 2;
        const CF_METAFILEPICT: u32 = 3;
        const CF_PALETTE: u32 = 9;
        const CF_ENHMETAFILE: u32 = 14;
        const CF_OWNERDISPLAY: u32 = 0x80;
        const CF_DSPBITMAP: u32 = 0x82;
        const CF_DSPMETAFILEPICT: u32 = 0x83;
        const CF_DSPENHMETAFILE: u32 = 0x8E;
        const CF_PRIVATEFIRST: u32 = 0x200;
        const CF_GDIOBJLAST: u32 = 0x3FF;
        !matches!(
            format,
            CF_BITMAP
                | CF_METAFILEPICT
                | CF_PALETTE
                | CF_ENHMETAFILE
                | CF_OWNERDISPLAY
                | CF_DSPBITMAP
                | CF_DSPMETAFILEPICT
                | CF_DSPENHMETAFILE
        ) && !(CF_PRIVATEFIRST..=CF_GDIOBJLAST).contains(&format)
    }

    /// Requires the clipboard to be open.
    fn save_clipboard() -> Vec<(u32, Vec<u8>)> {
        clipboard_win::raw::EnumFormats::new()
            .filter(|format| is_memory_format(*format))
            .filter_map(|format| {
                let mut data = Vec::new();
                clipboard_win::raw::get_vec(format, &mut data)
                    .ok()
                    .map(|_| (format, data))
            })
            .collect()
    }

    /// Requires the clipboard to be open. The restored copy is already in
    /// clipboard history, so it is not added again.
    fn restore_clipboard(saved: &[(u32, Vec<u8>)]) {
        let _ = clipboard_win::raw::empty();
        for (format, data) in saved {
            let _ = clipboard_win::raw::set_without_clear(*format, data);
        }
        mark_clipboard_item(&["CanIncludeInClipboardHistory", "CanUploadToCloudClipboard"]);
    }

    /// Windows reads these registered formats, set to a DWORD of 0, as
    /// "do not keep this item". Requires the clipboard to be open.
    fn mark_clipboard_item(format_names: &[&str]) {
        for name in format_names {
            if let Some(format) = clipboard_win::register_format(name) {
                let _ = clipboard_win::raw::set_without_clear(format.get(), &0u32.to_ne_bytes());
            }
        }
    }

    // ----- Low-level keyboard and mouse hooks -----

    fn note_user_input() {
        let foreground = unsafe { GetForegroundWindow() };
        if foreground == 0 || !is_own_process(foreground) {
            USER_INPUT_SINCE_WRITE.store(true, Ordering::SeqCst);
        }
    }

    // The hook procs must stay extremely small to avoid Windows timing them
    // out. They only flip atomics and swallow Space while the Ctrl+Space
    // gesture is active.
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
                            if HOLD_TO_TALK_ENABLED.load(Ordering::SeqCst) {
                                if !PRESS_TO_TALK_ACTIVE.swap(true, Ordering::SeqCst) {
                                    PRESS_TO_TALK_START.store(true, Ordering::SeqCst);
                                    DEBUG_EVENT_BITS.fetch_or(DBG_START_FLAG, Ordering::SeqCst);
                                }
                            } else {
                                TOGGLE_RECORDING.store(true, Ordering::SeqCst);
                                DEBUG_EVENT_BITS.fetch_or(DBG_TOGGLE_FLAG, Ordering::SeqCst);
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

                // Ctrl and Shift alone never move the cursor; every other key
                // (including Space typed without Ctrl) may.
                let modifier_only = matches!(
                    info.vk_code,
                    VK_CONTROL_CODE
                        | VK_LCONTROL_CODE
                        | VK_RCONTROL_CODE
                        | VK_SHIFT_CODE
                        | VK_LSHIFT_CODE
                        | VK_RSHIFT_CODE
                );
                if down && !modifier_only {
                    note_user_input();
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

    unsafe extern "system" fn ll_mouse_proc(n_code: i32, w_param: usize, l_param: isize) -> isize {
        if n_code >= 0
            && matches!(
                w_param as u32,
                WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
            )
        {
            let info = &*(l_param as *const MsLlHookStruct);
            // Clicks on the OpenWhisper pill do not move the target's cursor.
            if info.flags & LLMHF_INJECTED == 0 {
                let clicked = WindowFromPoint(info.pt);
                if clicked == 0 || !is_own_process(clicked) {
                    USER_INPUT_SINCE_WRITE.store(true, Ordering::SeqCst);
                }
            }
        }
        CallNextHookEx(
            MOUSE_HOOK_HANDLE.load(Ordering::Relaxed),
            n_code,
            w_param,
            l_param,
        )
    }

    /// Installs the low-level keyboard and mouse hooks and runs a blocking
    /// message pump. Blocks forever — must be called from a dedicated thread.
    pub fn install_input_hooks() {
        let h_mod = unsafe { GetModuleHandleW(std::ptr::null()) };
        let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, ll_keyboard_proc, h_mod, 0) };
        HOOK_HANDLE.store(hook, Ordering::Relaxed);
        let mouse_hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, ll_mouse_proc, h_mod, 0) };
        MOUSE_HOOK_HANDLE.store(mouse_hook, Ordering::Relaxed);

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

    /// A keystroke that types one UTF-16 code unit, whatever the keyboard layout.
    fn unicode_key(unit: u16, flags: u32) -> KbdInput {
        KbdInput {
            w_vk: 0,
            w_scan: unit,
            dw_flags: KEYEVENTF_UNICODE | flags,
            ..kbd(0, 0)
        }
    }

    /// Windows UI Automation: the only way to see that a browser or modern
    /// app field is a password field.
    mod uia {
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
        };
        use windows::Win32::UI::Accessibility::{
            CUIAutomation, CUIAutomation8, IUIAutomation, IUIAutomation2,
        };

        /// A hung target app must not stall dictation.
        const UIA_TIMEOUT_MS: u32 = 1000;

        fn automation() -> Option<IUIAutomation> {
            unsafe {
                // COM stays initialised on this worker thread; a thread that
                // is already initialised in another mode can still use UIA.
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                if let Ok(automation) = CoCreateInstance::<_, IUIAutomation2>(
                    &CUIAutomation8,
                    None,
                    CLSCTX_INPROC_SERVER,
                ) {
                    let _ = automation.SetConnectionTimeout(UIA_TIMEOUT_MS);
                    let _ = automation.SetTransactionTimeout(UIA_TIMEOUT_MS);
                    return Some(automation.into());
                }
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()
            }
        }

        /// `None` when UI Automation cannot tell.
        pub fn focused_element_is_password() -> Option<bool> {
            let automation = automation()?;
            unsafe {
                let element = automation.GetFocusedElement().ok()?;
                element
                    .CurrentIsPassword()
                    .ok()
                    .map(|value| value.as_bool())
            }
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
    const HWND_TOPMOST: isize = -1;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
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

    /// Re-insert the overlay at the very top of the always-on-top band.
    ///
    /// The window is created WS_EX_TOPMOST, but that only guarantees it sits
    /// above *non*-topmost windows — other topmost windows created after it
    /// (or brought forward later) can still stack on top. Every placement
    /// commit uses SWP_NOZORDER to avoid disturbing z-order during the hidden
    /// settling loop, so nothing otherwise re-raises the pill. The active
    /// listening pill must never be occluded, so we re-assert HWND_TOPMOST
    /// whenever the pill is shown or updated while visible. SWP_NOACTIVATE
    /// keeps it from stealing keyboard focus from the user's real window.
    pub fn reassert_topmost(hwnd: isize) {
        unsafe {
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            );
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
    "gpt-transcribe".to_string()
}

fn is_supported_transcription_model(model: &str) -> bool {
    matches!(model, "gpt-transcribe" | "gpt-live-transcribe")
}

/// Settings saved before the gpt-transcribe family move to its replacement.
fn migrate_transcription_model(model: &str) -> String {
    match model {
        "gpt-realtime-transcribe" => "gpt-live-transcribe".to_string(),
        model if is_supported_transcription_model(model) => model.to_string(),
        _ => default_transcription_model(),
    }
}

fn default_refine_enabled() -> bool {
    true
}

fn default_show_idle_pill() -> bool {
    true
}

fn default_hold_to_talk() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AppSettings {
    /// The key in use. It is kept in the OS credential store, never written
    /// to the settings file.
    #[serde(skip)]
    openai_api_key: String,
    /// A plain-text key in the settings file, from versions before the
    /// credential store. It is moved out on start and stays only while that
    /// move keeps failing.
    #[serde(
        default,
        rename = "openaiApiKey",
        skip_serializing_if = "String::is_empty"
    )]
    plain_openai_api_key: String,
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
    #[serde(default = "default_hold_to_talk")]
    hold_to_talk: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            openai_api_key: String::new(),
            plain_openai_api_key: String::new(),
            transcription_model: default_transcription_model(),
            transcription_prompt: default_transcription_prompt(),
            refine_enabled: true,
            refine_prompt: String::new(),
            show_idle_pill: true,
            hold_to_talk: true,
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
    hold_to_talk: bool,
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
    worker: Mutex<Option<Arc<WorkerClient>>>,
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
        hold_to_talk: settings.hold_to_talk,
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

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RecordingMetadata {
    id: String,
    created_at_ms: u128,
    name: String,
    source: String,
    mime_type: String,
    file_name: String,
    status: String,
    bytes: u64,
    transcript: String,
    error: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordingStartResponse {
    id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordingAudioResponse {
    mime_type: String,
    file_base64: String,
}

fn recordings_root(app_handle: &AppHandle) -> Result<PathBuf> {
    let mut root = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| anyhow!("Failed to resolve app data directory: {e}"))?;
    root.push("recordings");
    Ok(root)
}

fn recording_metadata_path(root: &std::path::Path, id: &str) -> PathBuf {
    root.join(id).join("metadata.json")
}

fn read_recording_metadata(root: &std::path::Path, id: &str) -> Result<RecordingMetadata> {
    let raw = fs::read_to_string(recording_metadata_path(root, id))?;
    Ok(serde_json::from_str(&raw)?)
}

fn write_recording_metadata(root: &std::path::Path, metadata: &RecordingMetadata) -> Result<()> {
    let path = recording_metadata_path(root, &metadata.id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_string_pretty(metadata)?)?;
    Ok(())
}

fn list_recording_metadata(root: &std::path::Path) -> Result<Vec<RecordingMetadata>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut recordings = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if let Ok(metadata) = read_recording_metadata(root, &entry.file_name().to_string_lossy()) {
            recordings.push(metadata);
        }
    }
    recordings.sort_by(|left, right| right.created_at_ms.cmp(&left.created_at_ms));
    Ok(recordings)
}

fn prune_recordings(root: &std::path::Path, keep: usize) -> Result<()> {
    for recording in list_recording_metadata(root)?.into_iter().skip(keep) {
        let path = root.join(recording.id);
        if path.starts_with(root) {
            fs::remove_dir_all(path)?;
        }
    }
    Ok(())
}

fn wav_header(pcm_bytes: u32, sample_rate: u32) -> [u8; 44] {
    let mut header = [0_u8; 44];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&(36_u32.saturating_add(pcm_bytes)).to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16_u32.to_le_bytes());
    header[20..22].copy_from_slice(&1_u16.to_le_bytes());
    header[22..24].copy_from_slice(&1_u16.to_le_bytes());
    header[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    header[28..32].copy_from_slice(&(sample_rate.saturating_mul(2)).to_le_bytes());
    header[32..34].copy_from_slice(&2_u16.to_le_bytes());
    header[34..36].copy_from_slice(&16_u16.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&pcm_bytes.to_le_bytes());
    header
}

fn append_pcm_chunk(
    root: &std::path::Path,
    recording_id: &str,
    chunk: &[u8],
    sample_rate: u32,
) -> Result<()> {
    let mut metadata = read_recording_metadata(root, recording_id)?;
    let path = root.join(recording_id).join(&metadata.file_name);
    let mut file = OpenOptions::new().read(true).write(true).open(&path)?;
    file.seek(SeekFrom::End(0))?;
    file.write_all(chunk)?;
    let pcm_bytes = file
        .metadata()?
        .len()
        .saturating_sub(44)
        .min(u32::MAX as u64) as u32;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&wav_header(pcm_bytes, sample_rate))?;
    file.flush()?;
    metadata.bytes = pcm_bytes as u64 + 44;
    write_recording_metadata(root, &metadata)?;
    Ok(())
}

fn safe_extension(name: &str, mime_type: &str) -> &'static str {
    let lowercase = name.to_ascii_lowercase();
    if lowercase.ends_with(".wav") || mime_type.contains("wav") {
        "wav"
    } else if lowercase.ends_with(".mp3") || mime_type.contains("mpeg") {
        "mp3"
    } else if lowercase.ends_with(".m4a") || mime_type.contains("m4a") {
        "m4a"
    } else if lowercase.ends_with(".mp4") || mime_type.contains("mp4") {
        "mp4"
    } else if lowercase.ends_with(".ogg") || mime_type.contains("ogg") {
        "ogg"
    } else {
        "webm"
    }
}

#[tauri::command]
fn recording_begin(
    app_handle: AppHandle,
    source: String,
    name: String,
) -> Result<RecordingStartResponse, String> {
    let root = recordings_root(&app_handle).map_err(|e| e.to_string())?;
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let id = Uuid::new_v4().to_string();
    let dir = root.join(&id);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(dir.join("audio.wav"), wav_header(0, 24_000)).map_err(|e| e.to_string())?;
    let metadata = RecordingMetadata {
        id: id.clone(),
        created_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0),
        name,
        source,
        mime_type: "audio/wav".to_string(),
        file_name: "audio.wav".to_string(),
        status: "recording".to_string(),
        bytes: 44,
        transcript: String::new(),
        error: String::new(),
    };
    write_recording_metadata(&root, &metadata).map_err(|e| e.to_string())?;
    prune_recordings(&root, RECORDING_RETENTION_COUNT).map_err(|e| e.to_string())?;
    Ok(RecordingStartResponse { id })
}

#[tauri::command]
fn recording_append_chunk(
    app_handle: AppHandle,
    recording_id: String,
    chunk_base64: String,
) -> Result<(), String> {
    let root = recordings_root(&app_handle).map_err(|e| e.to_string())?;
    let chunk = base64::engine::general_purpose::STANDARD
        .decode(chunk_base64)
        .map_err(|e| format!("Invalid audio chunk: {e}"))?;
    append_pcm_chunk(&root, &recording_id, &chunk, 24_000).map_err(|e| e.to_string())
}

#[tauri::command]
fn recording_import(
    app_handle: AppHandle,
    name: String,
    mime_type: String,
    file_base64: String,
) -> Result<RecordingStartResponse, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(file_base64)
        .map_err(|e| format!("Could not read imported audio: {e}"))?;
    if bytes.is_empty() {
        return Err("The selected audio file is empty".to_string());
    }
    let root = recordings_root(&app_handle).map_err(|e| e.to_string())?;
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let id = Uuid::new_v4().to_string();
    let extension = safe_extension(&name, &mime_type);
    let file_name = format!("audio.{extension}");
    let dir = root.join(&id);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(dir.join(&file_name), &bytes).map_err(|e| e.to_string())?;
    let metadata = RecordingMetadata {
        id: id.clone(),
        created_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0),
        name,
        source: "import".to_string(),
        mime_type,
        file_name,
        status: "saved".to_string(),
        bytes: bytes.len() as u64,
        transcript: String::new(),
        error: String::new(),
    };
    write_recording_metadata(&root, &metadata).map_err(|e| e.to_string())?;
    prune_recordings(&root, RECORDING_RETENTION_COUNT).map_err(|e| e.to_string())?;
    Ok(RecordingStartResponse { id })
}

#[tauri::command]
fn recording_finish(
    app_handle: AppHandle,
    recording_id: String,
    status: String,
    transcript: String,
    error: String,
) -> Result<(), String> {
    let root = recordings_root(&app_handle).map_err(|e| e.to_string())?;
    let mut metadata = read_recording_metadata(&root, &recording_id).map_err(|e| e.to_string())?;
    metadata.status = status;
    metadata.transcript = transcript;
    metadata.error = error;
    write_recording_metadata(&root, &metadata).map_err(|e| e.to_string())?;
    prune_recordings(&root, RECORDING_RETENTION_COUNT).map_err(|e| e.to_string())
}

#[tauri::command]
fn recording_list(app_handle: AppHandle) -> Result<Vec<RecordingMetadata>, String> {
    let root = recordings_root(&app_handle).map_err(|e| e.to_string())?;
    list_recording_metadata(&root).map_err(|e| e.to_string())
}

#[tauri::command]
fn recording_read(
    app_handle: AppHandle,
    recording_id: String,
) -> Result<RecordingAudioResponse, String> {
    let root = recordings_root(&app_handle).map_err(|e| e.to_string())?;
    let metadata = read_recording_metadata(&root, &recording_id).map_err(|e| e.to_string())?;
    let audio_path = root.join(recording_id).join(metadata.file_name);
    let bytes = fs::read(audio_path).map_err(|e| format!("Could not read saved recording: {e}"))?;
    if bytes.is_empty() {
        return Err("The saved recording is empty".to_string());
    }
    Ok(RecordingAudioResponse {
        mime_type: metadata.mime_type,
        file_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
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
    // Hold the lock only to find or start the worker, never while a request
    // runs, so a cancel or a settings save is not stuck behind a transcription.
    let worker = {
        let mut guard = state.worker.lock().await;
        match guard.as_ref() {
            Some(worker) if worker.is_alive() => worker.clone(),
            _ => {
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
                let worker = Arc::new(
                    WorkerClient::spawn(app_handle, openai_api_key, transcription_model)
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
            current.openai_api_key = api_key_store::replace(&CredentialStore, &key)
                .map_err(|e| format!("Could not save the API key securely: {e}"))?;
            current.plain_openai_api_key.clear();
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

    #[cfg(target_os = "windows")]
    text_inserter::set_hold_to_talk_enabled(updated.hold_to_talk);

    // Force a fresh worker spawn so new settings are applied. A request still
    // running keeps the old worker until it finishes.
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
    if !path.exists() {
        return Ok(AppSettings::default());
    }
    let raw = fs::read_to_string(&path)?;
    let mut settings: AppSettings = serde_json::from_str(&raw)?;
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
                std::thread::spawn(|| text_inserter::install_input_hooks());

                let poll_handle = app.handle().clone();
                std::thread::spawn(move || {
                    let mut last_hook_log = std::time::Instant::now();
                    let mut last_hook_event = String::new();
                    let mut monitor_transition = MonitorTransitionDebouncer::default();
                    loop {
                        text_inserter::track_foreground();
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
                            reveal_overlay_window(&poll_handle);
                            let _ = poll_handle.emit("press-to-talk-start", ());
                        }
                        if should_stop {
                            debug_log_line("shortcut", "emit press-to-talk-stop");
                            let _ = poll_handle.emit("press-to-talk-stop", ());
                        }
                        if should_toggle {
                            debug_log_line("shortcut", "emit toggle-recording");
                            text_inserter::track_foreground();
                            reveal_overlay_window(&poll_handle);
                            let _ = poll_handle.emit("toggle-recording", ());
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
            worker_append_audio_chunk,
            worker_finalize_session_audio,
            recording_begin,
            recording_append_chunk,
            recording_import,
            recording_finish,
            recording_list,
            recording_read,
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
    fn settings_file_never_contains_the_api_key() {
        let settings = AppSettings {
            openai_api_key: "sk-secret".into(),
            ..AppSettings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(!json.contains("sk-secret"));
        assert!(!json.contains("openaiApiKey"));
    }

    #[test]
    fn plain_key_from_older_versions_is_read_and_kept_until_moved() {
        let settings: AppSettings =
            serde_json::from_str(r#"{"openaiApiKey":"sk-legacy","holdToTalk":false}"#).unwrap();
        assert_eq!(settings.plain_openai_api_key, "sk-legacy");
        assert_eq!(settings.openai_api_key, "");
        assert!(!settings.hold_to_talk);
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains(r#""openaiApiKey":"sk-legacy""#));
    }

    #[test]
    fn recording_store_keeps_ten_readable_wavs() {
        let root = std::env::temp_dir().join(format!("openwhisper-store-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        for index in 0..12_u128 {
            let id = format!("recording-{index}");
            let directory = root.join(&id);
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("audio.wav"), wav_header(0, 24_000)).unwrap();
            write_recording_metadata(
                &root,
                &RecordingMetadata {
                    id: id.clone(),
                    created_at_ms: index,
                    name: format!("Recording {index}"),
                    source: "microphone".to_string(),
                    mime_type: "audio/wav".to_string(),
                    file_name: "audio.wav".to_string(),
                    status: "saved".to_string(),
                    bytes: 44,
                    transcript: String::new(),
                    error: String::new(),
                },
            )
            .unwrap();
        }

        prune_recordings(&root, RECORDING_RETENTION_COUNT).unwrap();
        let retained = list_recording_metadata(&root).unwrap();
        assert_eq!(retained.len(), RECORDING_RETENTION_COUNT);
        assert_eq!(retained.first().unwrap().id, "recording-11");
        assert_eq!(retained.last().unwrap().id, "recording-2");
        for recording in retained {
            let path = root.join(recording.id).join(recording.file_name);
            assert_eq!(&fs::read(path).unwrap()[0..4], b"RIFF");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn appended_backup_is_immediately_a_valid_wav() {
        let root = std::env::temp_dir().join(format!("openwhisper-wav-test-{}", Uuid::new_v4()));
        let id = "active";
        fs::create_dir_all(root.join(id)).unwrap();
        fs::write(root.join(id).join("audio.wav"), wav_header(0, 24_000)).unwrap();
        write_recording_metadata(
            &root,
            &RecordingMetadata {
                id: id.to_string(),
                created_at_ms: 1,
                name: "Active".to_string(),
                source: "microphone".to_string(),
                mime_type: "audio/wav".to_string(),
                file_name: "audio.wav".to_string(),
                status: "recording".to_string(),
                bytes: 44,
                transcript: String::new(),
                error: String::new(),
            },
        )
        .unwrap();

        append_pcm_chunk(&root, id, &[1, 2, 3, 4], 24_000).unwrap();
        let wav = fs::read(root.join(id).join("audio.wav")).unwrap();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 4);
        assert_eq!(&wav[44..], &[1, 2, 3, 4]);
        fs::remove_dir_all(root).unwrap();
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
        assert!(is_supported_transcription_model("gpt-transcribe"));
        assert!(is_supported_transcription_model("gpt-live-transcribe"));
        assert!(!is_supported_transcription_model("gpt-4o-mini-transcribe"));
        assert!(!is_supported_transcription_model("gpt-5.4"));
    }

    #[test]
    fn saved_legacy_transcription_models_migrate() {
        assert_eq!(migrate_transcription_model("gpt-4o-mini-transcribe"), "gpt-transcribe");
        assert_eq!(migrate_transcription_model("gpt-4o-transcribe"), "gpt-transcribe");
        assert_eq!(
            migrate_transcription_model("gpt-realtime-transcribe"),
            "gpt-live-transcribe"
        );
        assert_eq!(
            migrate_transcription_model("gpt-live-transcribe"),
            "gpt-live-transcribe"
        );
    }

    #[test]
    fn settings_without_idle_pill_preference_keep_showing_it() {
        let settings: AppSettings = serde_json::from_value(json!({})).unwrap();

        assert!(settings.show_idle_pill);
        assert!(frontend_settings_from(&settings).show_idle_pill);
    }

    #[test]
    fn settings_without_shortcut_preference_keep_hold_to_talk() {
        let settings: AppSettings = serde_json::from_value(json!({})).unwrap();

        assert!(settings.hold_to_talk);
        assert!(frontend_settings_from(&settings).hold_to_talk);
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
