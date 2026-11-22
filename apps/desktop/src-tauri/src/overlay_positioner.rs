//! Finds the monitor under the cursor and places the pill window with Win32
//! calls (Windows only). The placement maths is in `overlay_geometry.rs`.
//! Callers pass and get window and monitor handles as `isize`; they become
//! `HWND` and `HMONITOR` only here.

use openwhisper_core::overlay_geometry::{
    overlay_bottom_anchor, OverlayAnchor, OverlayPlacement, PlacementVisibility, Position, Size,
};
use std::ffi::c_void;
use std::mem::size_of;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONULL,
    MONITOR_DEFAULTTOPRIMARY, MONITOR_FROM_FLAGS,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowRect, SetWindowPos, HWND_TOPMOST, MONITORINFOF_PRIMARY,
    SET_WINDOW_POS_FLAGS, SWP_DEFERERASE, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOCOPYBITS,
    SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOREDRAW, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW,
};

fn to_hwnd(handle: isize) -> HWND {
    HWND(handle as *mut c_void)
}

fn monitor_anchor_at(point: POINT, fallback: MONITOR_FROM_FLAGS) -> Option<OverlayAnchor> {
    let monitor = unsafe { MonitorFromPoint(point, fallback) };
    if monitor.0.is_null() {
        return None;
    }

    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return None;
    }

    let mut dpi_x = 96;
    let mut dpi_y = 96;
    if unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }.is_err() {
        dpi_x = 96;
    }

    Some(OverlayAnchor {
        monitor_id: monitor.0 as isize,
        center_x: (info.rcWork.left + info.rcWork.right) / 2,
        bottom_y: overlay_bottom_anchor(
            info.rcMonitor.bottom,
            info.rcWork.bottom,
            info.dwFlags & MONITORINFOF_PRIMARY != 0,
            dpi_x,
        ),
        dpi: dpi_x,
    })
}

pub fn primary_monitor_anchor() -> Option<OverlayAnchor> {
    monitor_anchor_at(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY)
}

pub fn cursor_monitor_anchor() -> Option<OverlayAnchor> {
    let mut cursor = POINT::default();
    unsafe { GetCursorPos(&mut cursor) }.ok()?;
    // A cursor crossing a physical gap between displays belongs to no
    // monitor yet. Keep the committed placement until it is truly on one.
    monitor_anchor_at(cursor, MONITOR_DEFAULTTONULL)
}

/// The monitor under a screen point, or 0 between displays. One cheap call,
/// so the mouse hook can make it on every cursor move.
pub fn monitor_at(x: i32, y: i32) -> isize {
    unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONULL) }.0 as isize
}

pub fn set_window_placement(
    hwnd: isize,
    placement: OverlayPlacement,
    visibility: PlacementVisibility,
) -> Result<(), String> {
    let visibility_flags = match visibility {
        PlacementVisibility::Keep => SET_WINDOW_POS_FLAGS(0),
        PlacementVisibility::HideWithoutRedraw => {
            SWP_HIDEWINDOW | SWP_NOREDRAW | SWP_NOCOPYBITS | SWP_DEFERERASE
        }
        PlacementVisibility::Show => SWP_SHOWWINDOW,
    };
    let flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER | visibility_flags;
    unsafe {
        SetWindowPos(
            to_hwnd(hwnd),
            None,
            placement.position.x,
            placement.position.y,
            placement.size.width as i32,
            placement.size.height as i32,
            flags,
        )
    }
    .map_err(|_| "SetWindowPos failed while committing overlay placement".to_string())
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
    let _ = unsafe {
        SetWindowPos(
            to_hwnd(hwnd),
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        )
    };
}

/// Whether the window is at `expected`: its position, size and DPI.
pub fn placement_matches(hwnd: isize, expected: OverlayPlacement) -> Result<bool, String> {
    let hwnd = to_hwnd(hwnd);
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }
        .map_err(|_| "GetWindowRect failed while verifying overlay placement".to_string())?;
    Ok(Position::new(rect.left, rect.top) == expected.position
        && Size::new(
            rect.right.saturating_sub(rect.left) as u32,
            rect.bottom.saturating_sub(rect.top) as u32,
        ) == expected.size
        && unsafe { GetDpiForWindow(hwnd) } == expected.dpi)
}
