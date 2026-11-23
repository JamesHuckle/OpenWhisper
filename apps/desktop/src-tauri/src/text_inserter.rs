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

use openwhisper_core::foreground_app::ForegroundApp;
use openwhisper_core::overlay_geometry::cursor_entered_new_monitor;
use crate::overlay_positioner;
use openwhisper_core::paste_target::{self, Blocked, FinalWrite, LiveWrite, Observation};
use openwhisper_core::sync::lock;
use std::ffi::c_void;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HINSTANCE, HWND, LPARAM, LRESULT, MAX_PATH, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{
    CF_BITMAP, CF_DSPBITMAP, CF_DSPENHMETAFILE, CF_DSPMETAFILEPICT, CF_ENHMETAFILE, CF_GDIOBJLAST,
    CF_METAFILEPICT, CF_OWNERDISPLAY, CF_PALETTE, CF_PRIVATEFIRST,
};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, SetFocus, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_BACK, VK_CONTROL,
    VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
    VK_SPACE, VK_V,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetClassNameW, GetForegroundWindow, GetGUIThreadInfo, GetMessageW,
    GetWindowLongW, GetWindowThreadProcessId, IsWindow, SendMessageTimeoutW, SetForegroundWindow,
    SetWindowsHookExW, WindowFromPoint, ES_PASSWORD, EVENT_OBJECT_FOCUS, EVENT_SYSTEM_FOREGROUND,
    GUITHREADINFO, GWL_STYLE, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLMHF_INJECTED, MSG,
    MSLLHOOKSTRUCT, SMTO_ABORTIFHUNG, WH_KEYBOARD_LL, WH_MOUSE_LL, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_MOUSEMOVE,
    WM_NULL, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN,
};

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
/// Wakes the dispatcher thread in `run()` when a shortcut flag is set.
/// Nothing polls the flags.
static DISPATCHER_WAKE: OnceLock<Sender<()>> = OnceLock::new();
/// The monitor the cursor was last seen on (0 between displays).
static LAST_CURSOR_MONITOR: AtomicIsize = AtomicIsize::new(0);
/// Wakes the thread in `run()` that moves the pill when the cursor enters
/// another monitor, so it does not have to keep checking.
static MONITOR_WAKE: OnceLock<Sender<()>> = OnceLock::new();

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

const DBG_CTRL_DOWN: u32 = 1 << 0;
const DBG_CTRL_UP: u32 = 1 << 1;
const DBG_SPACE_DOWN: u32 = 1 << 2;
const DBG_SPACE_REPEAT: u32 = 1 << 3;
const DBG_SPACE_UP: u32 = 1 << 4;
const DBG_START_FLAG: u32 = 1 << 5;
const DBG_STOP_BY_CTRL: u32 = 1 << 6;
const DBG_STOP_BY_SPACE: u32 = 1 << 7;
const DBG_TOGGLE_FLAG: u32 = 1 << 8;
/// The name each debug bit has in the log, in log order.
const DEBUG_EVENT_NAMES: [(u32, &str); 9] = [
    (DBG_CTRL_DOWN, "ctrl_down"),
    (DBG_CTRL_UP, "ctrl_up"),
    (DBG_SPACE_DOWN, "space_down"),
    (DBG_SPACE_REPEAT, "space_repeat"),
    (DBG_SPACE_UP, "space_up"),
    (DBG_START_FLAG, "start_flag"),
    (DBG_STOP_BY_CTRL, "stop_flag_ctrl_up"),
    (DBG_STOP_BY_SPACE, "stop_flag_space_up"),
    (DBG_TOGGLE_FLAG, "toggle_flag"),
];

/// Ctrl+V is only queued: the target reads the clipboard when it handles
/// the key, and browsers read it on another thread a moment later.
const PASTE_HANDLED_TIMEOUT_MS: u32 = 1000;
const PASTE_SETTLE: Duration = Duration::from_millis(300);
const CLIPBOARD_OPEN_ATTEMPTS: usize = 10;
/// A key press; Windows has no flag for it, only for the release.
const KEY_DOWN: KEYBD_EVENT_FLAGS = KEYBD_EVENT_FLAGS(0);

/// Handles are kept as `isize` (in atomics and the session); they become
/// `HWND` and `HHOOK` only for a Windows call.
fn to_hwnd(handle: isize) -> HWND {
    HWND(handle as *mut c_void)
}

fn to_hhook(handle: isize) -> HHOOK {
    HHOOK(handle as *mut c_void)
}

fn foreground_window() -> isize {
    unsafe { GetForegroundWindow() }.0 as isize
}

fn is_own_process(hwnd: isize) -> bool {
    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(to_hwnd(hwnd), Some(&mut pid)) };
    pid == std::process::id()
}

/// The control with keyboard focus inside `hwnd`'s UI thread, or 0.
fn focused_control(hwnd: isize) -> isize {
    let thread_id = unsafe { GetWindowThreadProcessId(to_hwnd(hwnd), None) };
    let mut info = GUITHREADINFO {
        cbSize: size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    if thread_id != 0 && unsafe { GetGUIThreadInfo(thread_id, &mut info) }.is_ok() {
        info.hwndFocus.0 as isize
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
    let hwnd = foreground_window();
    if hwnd != 0 && !is_own_process(hwnd) {
        TRACKED_FOCUS_HWND.store(focused_control(hwnd), Ordering::Relaxed);
        TRACKED_HWND.store(hwnd, Ordering::Relaxed);
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
    *lock(&SESSION) = Some(session);
}

/// Returns the executable name (without `.exe`) of the tracked foreground
/// window, looked up once per process.
pub fn foreground_app_name() -> Option<String> {
    let hwnd = TRACKED_HWND.load(Ordering::Relaxed);
    if hwnd == 0 {
        return None;
    }
    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(to_hwnd(hwnd), Some(&mut pid)) };
    lock(&FOREGROUND_APP).name_for(pid, process_name)
}

fn process_name(pid: u32) -> Option<String> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = [0u16; MAX_PATH as usize];
    let mut size = MAX_PATH;
    let read = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut size,
        )
    };
    let _ = unsafe { CloseHandle(handle) };
    if read.is_err() || size == 0 {
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
    DEBUG_EVENT_NAMES
        .iter()
        .filter(|(bit, _)| bits & bit != 0)
        .map(|(_, name)| *name)
        .collect::<Vec<_>>()
        .join(",")
}

// ----- Checking the locked target before a write -----

fn key_held(vk: VIRTUAL_KEY) -> bool {
    let state = unsafe { GetAsyncKeyState(vk.0 as i32) };
    state < 0
}

fn restore_target_focus(session: &PasteSession) {
    let _ = unsafe { SetForegroundWindow(to_hwnd(session.hwnd)) };
    let focus_hwnd = session.focus_hwnd;
    if focus_hwnd != 0 && unsafe { IsWindow(Some(to_hwnd(focus_hwnd))) }.as_bool() {
        let target_thread = unsafe { GetWindowThreadProcessId(to_hwnd(session.hwnd), None) };
        let current_thread = unsafe { GetCurrentThreadId() };
        let attached = target_thread != 0
            && target_thread != current_thread
            && unsafe { AttachThreadInput(current_thread, target_thread, true) }.as_bool();
        let _ = unsafe { SetFocus(Some(to_hwnd(focus_hwnd))) };
        if attached {
            let _ = unsafe { AttachThreadInput(current_thread, target_thread, false) };
        }
    }
    std::thread::sleep(Duration::from_millis(80));
}

/// The target keeps focus, or gets it back when the user only clicked
/// OpenWhisper. Focus is never taken from another app the user moved to.
fn bring_target_forward(session: &PasteSession) -> bool {
    let foreground = foreground_window();
    if foreground == session.hwnd {
        return true;
    }
    if foreground != 0 && is_own_process(foreground) {
        restore_target_focus(session);
        return foreground_window() == session.hwnd;
    }
    false
}

/// Classic Win32 edit controls mark password fields with `ES_PASSWORD`.
fn is_win32_password_edit(hwnd: isize) -> bool {
    if hwnd == 0 || unsafe { GetWindowLongW(to_hwnd(hwnd), GWL_STYLE) } & ES_PASSWORD == 0 {
        return false;
    }
    let mut buf = [0u16; 64];
    let len = unsafe { GetClassNameW(to_hwnd(hwnd), &mut buf) };
    if len <= 0 {
        return false;
    }
    let class = String::from_utf16_lossy(&buf[..len as usize]).to_ascii_lowercase();
    class == "edit" || class.starts_with("richedit")
}

/// Also returns the input count it checked against.
fn observe(session: &PasteSession) -> (Observation, u64) {
    let input_count = USER_INPUT_COUNT.load(Ordering::SeqCst);
    let target_alive =
        session.hwnd != 0 && unsafe { IsWindow(Some(to_hwnd(session.hwnd))) }.as_bool();
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

/// Type one live piece of the transcript into the locked target.
pub fn type_live(text: &str) -> WriteOutcome {
    let mut guard = lock(&SESSION);
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
    let Some(session) = lock(&SESSION).take() else {
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
                // The user may have moved on while the clipboard was being
                // prepared; then nothing is deleted and the text stays copied.
                let (now, _) = observe(&session);
                let still = paste_target::decide_final(
                    &now,
                    &session.draft,
                    session.live_stopped.is_none(),
                    text,
                );
                if still != decision {
                    return false;
                }
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
fn send_inputs(inputs: &[INPUT]) -> bool {
    if inputs.is_empty() {
        return true;
    }
    let accepted = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    accepted == inputs.len() as u32
}

/// With Ctrl+Space held for hold-to-talk, typed letters would become
/// shortcuts and Backspace would delete whole words. Release Ctrl first;
/// the keyboard hook ignores injected keys, so the gesture keeps working.
fn release_held_ctrl() {
    let inputs: Vec<INPUT> = [VK_LCONTROL, VK_RCONTROL]
        .into_iter()
        .filter(|vk| key_held(*vk))
        .map(|vk| kbd(vk, KEYEVENTF_KEYUP))
        .collect();
    send_inputs(&inputs);
}

fn type_unicode(text: &str) -> bool {
    let units: Vec<u16> = text.encode_utf16().collect();
    for chunk in units.chunks(64) {
        let mut inputs = Vec::with_capacity(chunk.len() * 2);
        for &unit in chunk {
            inputs.push(unicode_key(unit, KEY_DOWN));
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
            inputs.push(kbd(VK_BACK, KEY_DOWN));
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
        kbd(VK_CONTROL, KEY_DOWN),
        kbd(VK_V, KEY_DOWN),
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
            to_hwnd(target),
            WM_NULL,
            WPARAM(0),
            LPARAM(0),
            SMTO_ABORTIFHUNG,
            PASTE_HANDLED_TIMEOUT_MS,
            Some(&mut result),
        )
    };
    std::thread::sleep(PASTE_SETTLE);
    handled.0 != 0
}

/// Clipboard formats whose data is plain memory. Bitmaps, metafiles and
/// palettes are GDI handles (their DIB forms are kept), and private
/// formats hold handles only their owner understands.
fn is_memory_format(format: u32) -> bool {
    let handle_formats = [
        CF_BITMAP,
        CF_METAFILEPICT,
        CF_PALETTE,
        CF_ENHMETAFILE,
        CF_OWNERDISPLAY,
        CF_DSPBITMAP,
        CF_DSPMETAFILEPICT,
        CF_DSPENHMETAFILE,
    ];
    let private_formats = u32::from(CF_PRIVATEFIRST.0)..=u32::from(CF_GDIOBJLAST.0);
    !handle_formats.iter().any(|cf| u32::from(cf.0) == format) && !private_formats.contains(&format)
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
    let foreground = foreground_window();
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
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if n_code >= 0 {
        let info = &*(l_param.0 as *const KBDLLHOOKSTRUCT);

        if !info.flags.contains(LLKHF_INJECTED) {
            let msg = w_param.0 as u32;
            // Virtual-key codes are 1 to 254.
            let vk = VIRTUAL_KEY(info.vkCode as u16);
            let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
            let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;

            match vk {
                VK_CONTROL | VK_LCONTROL | VK_RCONTROL => {
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
                        return LRESULT(1);
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
                        return LRESULT(1);
                    }

                    if SPACE_SWALLOWED.swap(false, Ordering::SeqCst) && up {
                        record_key_event(DBG_SPACE_UP);
                        if PRESS_TO_TALK_ACTIVE.swap(false, Ordering::SeqCst) {
                            PRESS_TO_TALK_STOP.store(true, Ordering::SeqCst);
                            record_key_event(DBG_STOP_BY_SPACE);
                        }
                        return LRESULT(1);
                    }
                }
                _ => {}
            }

            // Ctrl and Shift alone never move the cursor; every other key
            // (including Space typed without Ctrl) may.
            let modifier_only = matches!(
                vk,
                VK_CONTROL | VK_LCONTROL | VK_RCONTROL | VK_SHIFT | VK_LSHIFT | VK_RSHIFT
            );
            if down && !modifier_only {
                note_user_input();
            }
        }
    }
    CallNextHookEx(
        Some(to_hhook(HOOK_HANDLE.load(Ordering::Relaxed))),
        n_code,
        w_param,
        l_param,
    )
}

unsafe extern "system" fn ll_mouse_proc(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    // Every cursor move passes through here: only find its monitor and wake
    // the pill's follower when that changes, so the hook stays fast.
    if n_code >= 0 && w_param.0 as u32 == WM_MOUSEMOVE {
        let info = &*(l_param.0 as *const MSLLHOOKSTRUCT);
        let monitor = overlay_positioner::monitor_at(info.pt.x, info.pt.y);
        if cursor_entered_new_monitor(&LAST_CURSOR_MONITOR, monitor) {
            if let Some(wake) = MONITOR_WAKE.get() {
                let _ = wake.send(());
            }
        }
    }
    if n_code >= 0
        && matches!(
            w_param.0 as u32,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
        )
    {
        let info = &*(l_param.0 as *const MSLLHOOKSTRUCT);
        // Clicks on the OpenWhisper pill do not move the target's cursor.
        if info.flags & LLMHF_INJECTED == 0 {
            let clicked = WindowFromPoint(info.pt).0 as isize;
            if clicked == 0 || !is_own_process(clicked) {
                USER_INPUT_COUNT.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
    CallNextHookEx(
        Some(to_hhook(MOUSE_HOOK_HANDLE.load(Ordering::Relaxed))),
        n_code,
        w_param,
        l_param,
    )
}

/// Windows calls this on this thread's message pump whenever another
/// program's window comes to the front or a control in it takes focus.
unsafe extern "system" fn focus_event_proc(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    track_foreground();
}

/// Installs the low-level keyboard and mouse hooks and the focus-change
/// hooks, then runs a blocking message pump. `wake` is signalled whenever
/// a shortcut flag is set, and `monitor_wake` whenever the cursor enters
/// another monitor. Blocks
/// forever — must be called from a dedicated thread.
pub fn install_input_hooks(wake: Sender<()>, monitor_wake: Sender<()>) {
    let _ = DISPATCHER_WAKE.set(wake);
    let _ = MONITOR_WAKE.set(monitor_wake);
    let h_mod = unsafe { GetModuleHandleW(PCWSTR::null()) }
        .ok()
        .map(HINSTANCE::from);
    // A hook that failed is stored as 0, which CallNextHookEx accepts.
    let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(ll_keyboard_proc), h_mod, 0) };
    HOOK_HANDLE.store(hook.map_or(0, |h| h.0 as isize), Ordering::Relaxed);
    let mouse_hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(ll_mouse_proc), h_mod, 0) };
    MOUSE_HOOK_HANDLE.store(mouse_hook.map_or(0, |h| h.0 as isize), Ordering::Relaxed);
    for event in [EVENT_SYSTEM_FOREGROUND, EVENT_OBJECT_FOCUS] {
        unsafe {
            SetWinEventHook(
                event,
                event,
                None,
                Some(focus_event_proc),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };
    }
    track_foreground();

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {}
}

fn kbd(vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                dwFlags: flags,
                ..Default::default()
            },
        },
    }
}

/// A keystroke that types one UTF-16 code unit, whatever the keyboard layout.
fn unicode_key(unit: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wScan: unit,
                dwFlags: KEYEVENTF_UNICODE | flags,
                ..Default::default()
            },
        },
    }
}

/// Windows UI Automation: the only way to see that a browser or modern
/// app field is a password field.
///
/// Each thread that writes keeps one COM setup and one automation client for
/// its whole life (`automation_client` has the reuse rules); the focused
/// element and its password flag are still read again on every write.
mod uia {
    use openwhisper_core::automation_client::ReusedClient;
    use std::cell::RefCell;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, CUIAutomation8, IUIAutomation, IUIAutomation2,
    };

    /// A hung target app must not stall dictation.
    const UIA_TIMEOUT_MS: u32 = 1000;

    /// COM joined once by this thread, and left when the thread ends.
    struct ComThread {
        joined: bool,
    }

    impl ComThread {
        fn join() -> Self {
            // A thread already initialised in another mode can still use
            // UIA; that call failed, so it is not balanced on exit.
            let joined = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
            Self { joined }
        }
    }

    impl Drop for ComThread {
        fn drop(&mut self) {
            if self.joined {
                unsafe { CoUninitialize() };
            }
        }
    }

    struct Context {
        // Fields drop in order: the client is released before COM is left.
        automation: ReusedClient<IUIAutomation, fn() -> Option<IUIAutomation>>,
        _com: ComThread,
    }

    thread_local! {
        static CONTEXT: RefCell<Context> = RefCell::new(Context {
            _com: ComThread::join(),
            automation: ReusedClient::new(create_automation),
        });
    }

    fn create_automation() -> Option<IUIAutomation> {
        unsafe {
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
        CONTEXT
            .try_with(|context| {
                context.borrow_mut().automation.query(|automation| unsafe {
                    let element = automation.GetFocusedElement().map_err(|e| e.code().0)?;
                    element
                        .CurrentIsPassword()
                        .map(|value| value.as_bool())
                        .map_err(|e| e.code().0)
                })
            })
            .ok()
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_events_are_named_in_bit_order() {
        assert_eq!(describe_debug_events(0), "");
        assert_eq!(describe_debug_events(DBG_SPACE_UP), "space_up");
        assert_eq!(
            describe_debug_events(DBG_TOGGLE_FLAG | DBG_CTRL_DOWN | DBG_STOP_BY_SPACE),
            "ctrl_down,stop_flag_space_up,toggle_flag"
        );
        assert_eq!(
            describe_debug_events(u32::MAX),
            "ctrl_down,ctrl_up,space_down,space_repeat,space_up,start_flag,\
             stop_flag_ctrl_up,stop_flag_space_up,toggle_flag"
        );
    }
}
