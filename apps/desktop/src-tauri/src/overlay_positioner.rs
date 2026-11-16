//! Finds the monitor under the cursor and places the pill window with Win32
//! calls (Windows only). The placement maths is in `overlay_geometry.rs`.

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
