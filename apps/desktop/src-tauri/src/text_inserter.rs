//! Writes dictated text into the field focused when recording began
//! (Windows only).
//!
//! A background loop tracks the last focused window outside OpenWhisper, so
//! that clicking the pill still knows where the user was. When a recording
//! starts, `capture_target` locks that window and its focused control for the
//! whole recording; the loop never changes a locked target. Before every write
//! the target is checked again (same window and control, not a password field,
//! no key press or click since the last write) and `paste_target` decides what
//! may be written. Live pieces are typed as Unicode keystrokes. The final text
//! is pasted through the clipboard, which is put back afterwards; the temporary
//! text is kept out of clipboard history and cloud sync.

use crate::foreground_app::ForegroundApp;
use crate::paste_target::{self, Blocked, FinalWrite, LiveWrite, Observation};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};
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
/// Counts the keys the user presses and clicks outside OpenWhisper. A
/// session remembers the count its last check accepted, so a key pressed
/// while a piece is being typed is still seen at the next check.
static USER_INPUT_COUNT: AtomicU64 = AtomicU64::new(0);
static SESSION: Mutex<Option<PasteSession>> = Mutex::new(None);
static FOREGROUND_APP: Mutex<ForegroundApp> = Mutex::new(ForegroundApp::new());
/// Wakes the dispatcher thread in `run()` when a shortcut flag is set or
/// the followed window changes. Nothing polls the flags.
static DISPATCHER_WAKE: OnceLock<Sender<()>> = OnceLock::new();

/// The target locked for one recording and the live draft typed into it.
struct PasteSession {
    hwnd: isize,
    focus_hwnd: isize,
    draft: String,
    live_stopped: Option<Blocked>,
    input_seen: u64,
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
const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const EVENT_OBJECT_FOCUS: u32 = 0x8005;
const WINEVENT_OUTOFCONTEXT: u32 = 0x0000;
const WINEVENT_SKIPOWNPROCESS: u32 = 0x0002;
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
    fn SetWinEventHook(
        event_min: u32,
        event_max: u32,
        hmod_win_event_proc: isize,
        pfn_win_event_proc: unsafe extern "system" fn(isize, u32, isize, i32, i32, u32, u32),
        id_process: u32,
        id_thread: u32,
        dw_flags: u32,
    ) -> isize;
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

fn wake_dispatcher() {
    if let Some(wake) = DISPATCHER_WAKE.get() {
        let _ = wake.send(());
    }
}

/// Follow the current foreground window, ignoring our own process. Runs
/// whenever Windows reports a focus change (see `install_input_hooks`);
/// it never changes the target locked for a recording.
pub fn track_foreground() {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd != 0 && !is_own_process(hwnd) {
        TRACKED_FOCUS_HWND.store(focused_control(hwnd), Ordering::Relaxed);
        if TRACKED_HWND.swap(hwnd, Ordering::Relaxed) != hwnd {
            wake_dispatcher();
        }
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
        input_seen: USER_INPUT_COUNT.load(Ordering::SeqCst),
    };
    *SESSION.lock().unwrap_or_else(|e| e.into_inner()) = Some(session);
}

fn lock_foreground_app() -> std::sync::MutexGuard<'static, ForegroundApp> {
    FOREGROUND_APP.lock().unwrap_or_else(|e| e.into_inner())
}

/// Returns the executable name (without `.exe`) of the tracked foreground
/// window, looked up once per process.
pub fn foreground_app_name() -> Option<String> {
    let hwnd = TRACKED_HWND.load(Ordering::Relaxed);
    if hwnd == 0 {
        return None;
    }
    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    lock_foreground_app().name_for(pid, process_name)
}

/// The foreground program's name when it changed since the last call,
/// for the settings menu.
pub fn foreground_app_change() -> Option<Option<String>> {
    let name = foreground_app_name();
    lock_foreground_app().report(name)
}

fn process_name(pid: u32) -> Option<String> {
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

/// Also returns the input count it checked against.
fn observe(session: &PasteSession) -> (Observation, u64) {
    let input_count = USER_INPUT_COUNT.load(Ordering::SeqCst);
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
    let observation = Observation {
        target_alive,
        foreground_is_target,
        same_field,
        is_password,
        user_input_since_write: input_count != session.input_seen,
        shortcut_modifier_held: [VK_LMENU, VK_RMENU, VK_LWIN, VK_RWIN]
            .into_iter()
            .any(key_held),
    };
    (observation, input_count)
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
    let (observation, input_count) = observe(session);
    match paste_target::decide_live(&observation, !session.draft.is_empty()) {
        LiveWrite::Type => {
            let typed = paste_target::typeable_text(text);
            release_held_ctrl();
            if !type_unicode(&typed) {
                // The draft is unknown now, so it is never deleted; the
                // final text is pasted after it.
                session.live_stopped = Some(Blocked::InputRefused);
                return WriteOutcome::Stopped(Blocked::InputRefused);
            }
            session.draft.push_str(&typed);
            // Only input from before the check is accepted; anything
            // pressed since then is seen at the next check.
            session.input_seen = input_count;
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
    let (observation, _) = observe(&session);
    let decision = paste_target::decide_final(
        &observation,
        &session.draft,
        session.live_stopped.is_none(),
        text,
    );
    match decision {
        FinalWrite::Replace { delete_chars } => {
            // The draft is deleted only once the final text is ready on
            // the clipboard, so a clipboard failure leaves it in place.
            let pasted = paste_restoring_clipboard(text, session.hwnd, || {
                release_held_ctrl();
                press_backspace(delete_chars)
            })?;
            Ok(if pasted {
                WriteOutcome::Replaced
            } else {
                WriteOutcome::Copied(Blocked::InputRefused)
            })
        }
        FinalWrite::Paste => {
            let pasted = paste_restoring_clipboard(text, session.hwnd, || true)?;
            Ok(if pasted {
                WriteOutcome::Pasted
            } else {
                WriteOutcome::Copied(Blocked::InputRefused)
            })
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

/// True when Windows accepted every key.
fn send_inputs(inputs: &[KbdInput]) -> bool {
    if inputs.is_empty() {
        return true;
    }
    let accepted = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<KbdInput>() as i32,
        )
    };
    accepted == inputs.len() as u32
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

fn type_unicode(text: &str) -> bool {
    let units: Vec<u16> = text.encode_utf16().collect();
    for chunk in units.chunks(64) {
        let mut inputs = Vec::with_capacity(chunk.len() * 2);
        for &unit in chunk {
            inputs.push(unicode_key(unit, 0));
            inputs.push(unicode_key(unit, KEYEVENTF_KEYUP));
        }
        if !send_inputs(&inputs) {
            return false;
        }
    }
    true
}

fn press_backspace(count: usize) -> bool {
    let mut remaining = count;
    while remaining > 0 {
        let chunk_size = remaining.min(128);
        let mut inputs = Vec::with_capacity(chunk_size * 2);
        for _ in 0..chunk_size {
            inputs.push(kbd(VK_BACK, 0));
            inputs.push(kbd(VK_BACK, KEYEVENTF_KEYUP));
        }
        if !send_inputs(&inputs) {
            return false;
        }
        remaining -= chunk_size;
    }
    true
}

fn paste_shortcut() -> bool {
    send_inputs(&[
        kbd(VK_CONTROL, 0),
        kbd(VK_V, 0),
        kbd(VK_V, KEYEVENTF_KEYUP),
        kbd(VK_CONTROL, KEYEVENTF_KEYUP),
    ])
}

// ----- Clipboard paste that puts the user's clipboard back -----

/// Paste `text` with Ctrl+V, then restore what the user had copied,
/// unless something else changed the clipboard in the meantime.
/// `before_paste` runs once `text` is on the clipboard. Returns false,
/// leaving `text` on the clipboard, when `before_paste` or Windows refused
/// the keys, or the target did not handle them in time.
fn paste_restoring_clipboard(
    text: &str,
    target: isize,
    before_paste: impl FnOnce() -> bool,
) -> Result<bool, String> {
    let (saved, ours) = {
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
        // Read while the clipboard is still open, so it is our item's number.
        (saved, clipboard_win::raw::seq_num())
    };
    if !before_paste() || !paste_shortcut() {
        return Ok(false);
    }
    if !wait_until_pasted(target) {
        // The target may still paste later; it must find the transcript.
        return Ok(false);
    }
    if let Ok(_open) = clipboard_win::Clipboard::new_attempts(CLIPBOARD_OPEN_ATTEMPTS) {
        // Checked while the clipboard is open, so no other app can copy
        // between this check and the restore.
        if clipboard_win::raw::seq_num() == ours {
            restore_clipboard(&saved);
        }
    }
    Ok(true)
}

/// False when the target did not handle its waiting input in time.
fn wait_until_pasted(target: isize) -> bool {
    std::thread::sleep(Duration::from_millis(50));
    let mut result = 0usize;
    let handled = unsafe {
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
    handled != 0
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
        USER_INPUT_COUNT.fetch_add(1, Ordering::SeqCst);
    }
}

/// Notes a Ctrl or Space event for the debug log and wakes the dispatcher,
/// which then reads the shortcut flags set just before.
fn record_key_event(bit: u32) {
    DEBUG_EVENT_BITS.fetch_or(bit, Ordering::SeqCst);
    wake_dispatcher();
}

// The hook procs must stay extremely small to avoid Windows timing them
// out. They only flip atomics, wake the dispatcher and swallow Space while
// the Ctrl+Space gesture is active.
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
                        record_key_event(DBG_CTRL_DOWN);
                    } else if up {
                        CTRL_HELD.store(false, Ordering::SeqCst);
                        record_key_event(DBG_CTRL_UP);
                        if PRESS_TO_TALK_ACTIVE.swap(false, Ordering::SeqCst) {
                            PRESS_TO_TALK_STOP.store(true, Ordering::SeqCst);
                            record_key_event(DBG_STOP_BY_CTRL);
                        }
                    }
                }
                VK_SPACE => {
                    // Check repeat first: if we're already in press-to-talk, subsequent downs are key repeat
                    if SPACE_SWALLOWED.load(Ordering::SeqCst) && down {
                        record_key_event(DBG_SPACE_REPEAT);
                        return 1;
                    }

                    if down && CTRL_HELD.load(Ordering::SeqCst) {
                        SPACE_SWALLOWED.store(true, Ordering::SeqCst);
                        record_key_event(DBG_SPACE_DOWN);
                        if HOLD_TO_TALK_ENABLED.load(Ordering::SeqCst) {
                            if !PRESS_TO_TALK_ACTIVE.swap(true, Ordering::SeqCst) {
                                PRESS_TO_TALK_START.store(true, Ordering::SeqCst);
                                record_key_event(DBG_START_FLAG);
                            }
                        } else {
                            TOGGLE_RECORDING.store(true, Ordering::SeqCst);
                            record_key_event(DBG_TOGGLE_FLAG);
                        }
                        return 1;
                    }

                    if SPACE_SWALLOWED.swap(false, Ordering::SeqCst) && up {
                        record_key_event(DBG_SPACE_UP);
                        if PRESS_TO_TALK_ACTIVE.swap(false, Ordering::SeqCst) {
                            PRESS_TO_TALK_STOP.store(true, Ordering::SeqCst);
                            record_key_event(DBG_STOP_BY_SPACE);
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
                USER_INPUT_COUNT.fetch_add(1, Ordering::SeqCst);
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

/// Windows calls this on this thread's message pump whenever another
/// program's window comes to the front or a control in it takes focus.
unsafe extern "system" fn focus_event_proc(
    _hook: isize,
    _event: u32,
    _hwnd: isize,
    _id_object: i32,
    _id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    track_foreground();
}

/// Installs the low-level keyboard and mouse hooks and the focus-change
/// hooks, then runs a blocking message pump. `wake` is signalled whenever
/// a shortcut flag is set or the followed window changes. Blocks forever —
/// must be called from a dedicated thread.
pub fn install_input_hooks(wake: Sender<()>) {
    let _ = DISPATCHER_WAKE.set(wake);
    let h_mod = unsafe { GetModuleHandleW(std::ptr::null()) };
    let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, ll_keyboard_proc, h_mod, 0) };
    HOOK_HANDLE.store(hook, Ordering::Relaxed);
    let mouse_hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, ll_mouse_proc, h_mod, 0) };
    MOUSE_HOOK_HANDLE.store(mouse_hook, Ordering::Relaxed);
    for event in [EVENT_SYSTEM_FOREGROUND, EVENT_OBJECT_FOCUS] {
        unsafe {
            SetWinEventHook(
                event,
                event,
                0,
                focus_event_proc,
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };
    }
    track_foreground();

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
