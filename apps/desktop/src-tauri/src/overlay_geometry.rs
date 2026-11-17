//! Where the pill window goes: its anchor at the bottom centre of a monitor,
//! its size and position in physical pixels at that monitor's DPI, when a
//! move to another monitor is committed, when a cursor move should wake the
//! thread that follows the cursor, and whether showing it takes focus.
//! The Win32 calls that apply a placement are in `overlay_positioner.rs`.
//!
//! This file needs no Tauri, so its tests also run in `core-tests`.

use std::sync::atomic::{AtomicIsize, Ordering};

/// A window position in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

impl Position {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// A window size in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

const SECONDARY_MONITOR_VERTICAL_OFFSET_LOGICAL: f64 = 2.0;

pub fn overlay_bottom_anchor(monitor_bottom: i32, work_bottom: i32, is_primary: bool, dpi: u32) -> i32 {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayAnchor {
    pub monitor_id: isize,
    pub center_x: i32,
    pub bottom_y: i32,
    pub dpi: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayLayoutSpec {
    pub width: f64,
    pub height: f64,
    pub anchor_offset_y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayPlacement {
    pub position: Position,
    pub size: Size,
    pub dpi: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementVisibility {
    Keep,
    HideWithoutRedraw,
    Show,
}

/// Consecutive same-monitor checks needed before the pill moves.
const MONITOR_SWITCH_STABLE_SAMPLES: u8 = 2;

#[derive(Default)]
pub struct MonitorTransitionDebouncer {
    candidate: Option<OverlayAnchor>,
    samples: u8,
}

impl MonitorTransitionDebouncer {
    pub fn observe(
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

    pub fn reset(&mut self) {
        self.candidate = None;
        self.samples = 0;
    }
}

/// Called by the mouse hook on every cursor move with the monitor under the
/// cursor (0 when it is between displays). Remembers it in `last` and says
/// whether the cursor has just entered a monitor other than the one it was
/// last on, which is the only time the pill may need to move. Moves on the
/// same monitor, the large majority, must not wake anything.
pub fn cursor_entered_new_monitor(last: &AtomicIsize, monitor: isize) -> bool {
    last.swap(monitor, Ordering::Relaxed) != monitor && monitor != 0
}

fn overlay_window_position_for_anchor(
    anchor: OverlayAnchor,
    physical_width: i32,
    physical_height: i32,
) -> Position {
    let x = anchor.center_x.saturating_sub(physical_width / 2);
    let y = anchor.bottom_y.saturating_sub(physical_height);

    Position::new(x, y)
}

fn overlay_window_position_for_anchor_offset(
    anchor: OverlayAnchor,
    physical_width: i32,
    physical_height: i32,
    physical_bottom_offset: i32,
) -> Position {
    let mut position = overlay_window_position_for_anchor(anchor, physical_width, physical_height);
    position.y = position.y.saturating_add(physical_bottom_offset);
    position
}

fn physical_dimension(logical: f64, dpi: u32) -> u32 {
    (logical * dpi as f64 / 96.0).round().max(1.0) as u32
}

pub fn overlay_placement(anchor: OverlayAnchor, layout: OverlayLayoutSpec) -> OverlayPlacement {
    let size = Size::new(
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

/// Whether showing the pill may take keyboard focus.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverlayRevealIntent {
    Passive,
    Interactive,
}

impl OverlayRevealIntent {
    pub fn should_focus(self) -> bool {
        matches!(self, Self::Interactive)
    }
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

        assert_eq!(collapsed, Position::new(-1330, 1402));
        assert_eq!(expanded, Position::new(-1460, 910));
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
            Position::new(866, -82)
        );
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

        assert_eq!(at_125.size, Size::new(115, 35));
        assert_eq!(at_125.position, Position::new(943, 868));
        assert_eq!(at_200.size, Size::new(184, 56));
        assert_eq!(at_200.position, Position::new(1908, 1648));
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
    fn cursor_moves_wake_the_follower_only_when_entering_another_monitor() {
        let last = AtomicIsize::new(0);
        let wakes = |moves: &[isize]| {
            moves
                .iter()
                .filter(|&&monitor| cursor_entered_new_monitor(&last, monitor))
                .count()
        };

        // The first move tells the follower where the cursor is.
        assert_eq!(wakes(&[1]), 1);
        // Moving about on the same monitor never wakes it.
        assert_eq!(wakes(&[1; 1000]), 0);
        // Entering another monitor wakes it once, however long it stays.
        assert_eq!(wakes(&[2; 1000]), 1);
        // Crossing a gap between displays wakes it only on arrival.
        assert_eq!(wakes(&[0, 0, 3, 3]), 1);
        // A gap, then the same monitor again, wakes it once more, so a
        // follower that stopped while the cursor was in the gap catches up.
        assert_eq!(wakes(&[0, 3]), 1);
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
