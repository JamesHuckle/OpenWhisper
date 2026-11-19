//! Where the pill window goes: its anchor at the bottom centre of a monitor,
//! its size and position in physical pixels at that monitor's DPI, when a
//! move to another monitor is committed and how it is made while hidden,
//! when a cursor move should wake the thread that follows the cursor, and
//! whether showing it takes focus.
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

/// Moves the pill to `target` on another monitor. A monitor move can
/// dispatch WM_DPICHANGED after SetWindowPos returns, so every in-between
/// rectangle is kept hidden and non-redrawing, and only one already-settled
/// rectangle is shown. `commit` applies a placement, `matches` says whether
/// the window is at a placement, and `wait` pauses between checks.
///
/// `restore` is the pill's last good placement when it was visible before
/// the move, and `None` when it was hidden. A visible pill is shown at
/// `target` once it settles; if any step fails after the pill was hidden, it
/// is shown again at `restore` and the move's error is returned, so a failed
/// move never leaves a visible pill hidden. A hidden pill is never shown.
/// Returns how many times Windows moved the pill off `target` while hidden.
pub fn move_overlay_hidden(
    target: OverlayPlacement,
    restore: Option<OverlayPlacement>,
    mut commit: impl FnMut(OverlayPlacement, PlacementVisibility) -> Result<(), String>,
    mut matches: impl FnMut(OverlayPlacement) -> Result<bool, String>,
    mut wait: impl FnMut(),
) -> Result<u8, String> {
    let moved = settle_hidden(target, &mut commit, &mut matches, &mut wait).and_then(|corrections| {
        let corrections = corrections.ok_or_else(|| {
            "Windows did not settle the overlay at its committed DPI placement".to_string()
        })?;
        if restore.is_some() {
            commit(target, PlacementVisibility::Show)?;
        }
        Ok(corrections)
    });
    match (moved, restore) {
        (Err(err), Some(restore)) => Err(match commit(restore, PlacementVisibility::Show) {
            Ok(()) => err,
            Err(restore_err) => format!("{err}; showing the pill again also failed: {restore_err}"),
        }),
        (moved, _) => moved,
    }
}

/// Commits `target` hidden and without redraw, then checks it up to 12
/// times, `wait`ing before each check and committing it hidden again
/// whenever Windows moved it, until two checks in a row find it in place.
/// Returns how many times it was committed again, or `None` if the window
/// is still off `target` at the end; each caller words that error itself.
pub fn settle_hidden(
    target: OverlayPlacement,
    mut commit: impl FnMut(OverlayPlacement, PlacementVisibility) -> Result<(), String>,
    mut matches: impl FnMut(OverlayPlacement) -> Result<bool, String>,
    mut wait: impl FnMut(),
) -> Result<Option<u8>, String> {
    commit(target, PlacementVisibility::HideWithoutRedraw)?;
    let mut stable_samples = 0u8;
    let mut correction_count = 0u8;
    for _ in 0..12 {
        wait();
        if matches(target)? {
            stable_samples = stable_samples.saturating_add(1);
            if stable_samples >= 2 {
                break;
            }
        } else {
            stable_samples = 0;
            correction_count = correction_count.saturating_add(1);
            commit(target, PlacementVisibility::HideWithoutRedraw)?;
        }
    }
    Ok(matches(target)?.then_some(correction_count))
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

    /// A pill window that moves where it is told, except that Windows moves
    /// it off its placement on the next `drifts` checks (as WM_DPICHANGED
    /// does), and the step named in `fails` returns an error.
    struct FakeWindow {
        placement: OverlayPlacement,
        visible: bool,
        drifts: usize,
        fails: Option<FakeFailure>,
        shows: Vec<OverlayPlacement>,
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum FakeFailure {
        Hide,
        Check,
        Show,
        Every,
    }

    fn placement_at(x: i32, dpi: u32) -> OverlayPlacement {
        OverlayPlacement {
            position: Position::new(x, 900),
            size: Size::new(100, 28),
            dpi,
        }
    }

    fn move_fake(
        visible: bool,
        drifts: usize,
        fails: Option<FakeFailure>,
    ) -> (Result<u8, String>, FakeWindow, OverlayPlacement, OverlayPlacement) {
        let last_good = placement_at(0, 96);
        let target = placement_at(2000, 144);
        let window = std::cell::RefCell::new(FakeWindow {
            placement: last_good,
            visible,
            drifts,
            fails,
            shows: Vec::new(),
        });
        let moved = move_overlay_hidden(
            target,
            visible.then_some(last_good),
            |placement, visibility| {
                let mut window = window.borrow_mut();
                let failing = match visibility {
                    PlacementVisibility::HideWithoutRedraw => FakeFailure::Hide,
                    _ => FakeFailure::Show,
                };
                if window.fails == Some(failing) || window.fails == Some(FakeFailure::Every) {
                    return Err(format!("SetWindowPos failed ({failing:?})"));
                }
                window.placement = placement;
                match visibility {
                    PlacementVisibility::Keep => {}
                    PlacementVisibility::HideWithoutRedraw => window.visible = false,
                    PlacementVisibility::Show => {
                        window.visible = true;
                        window.shows.push(placement);
                    }
                }
                Ok(())
            },
            |placement| {
                let mut window = window.borrow_mut();
                if window.fails == Some(FakeFailure::Check) {
                    return Err("GetWindowRect failed while verifying overlay placement".to_string());
                }
                if window.drifts > 0 {
                    window.drifts -= 1;
                    window.placement.size = Size::new(150, 42);
                }
                Ok(window.placement == placement)
            },
            || {},
        );
        (moved, window.into_inner(), last_good, target)
    }

    #[test]
    fn visible_pill_is_shown_once_at_its_settled_target() {
        let (moved, window, _, target) = move_fake(true, 3, None);

        assert_eq!(moved, Ok(3));
        assert!(window.visible);
        assert_eq!(window.placement, target);
        assert_eq!(window.shows, vec![target]);
    }

    #[test]
    fn pill_hidden_on_purpose_stays_hidden_after_a_move() {
        let (moved, window, _, target) = move_fake(false, 1, None);

        assert_eq!(moved, Ok(1));
        assert!(!window.visible);
        assert_eq!(window.placement, target);
        assert!(window.shows.is_empty());
    }

    #[test]
    fn visible_pill_comes_back_at_its_last_good_placement_when_a_move_fails() {
        // Bug: a visible pill that failed to settle on another monitor was
        // left hidden, and the retry then treated it as hidden on purpose.
        let failures = [
            (usize::MAX, None, "Windows did not settle the overlay at its committed DPI placement"),
            (0, Some(FakeFailure::Hide), "SetWindowPos failed (Hide)"),
            (0, Some(FakeFailure::Check), "GetWindowRect failed while verifying overlay placement"),
        ];
        for (drifts, fails, error) in failures {
            let (moved, window, last_good, _) = move_fake(true, drifts, fails);

            assert_eq!(moved, Err(error.to_string()), "{fails:?}");
            assert!(window.visible, "{fails:?}");
            assert_eq!(window.placement, last_good, "{fails:?}");
            assert_eq!(window.shows, vec![last_good], "{fails:?}");
        }
    }

    #[test]
    fn pill_hidden_on_purpose_is_never_shown_when_a_move_fails() {
        let failures = [
            (usize::MAX, None),
            (0, Some(FakeFailure::Hide)),
            (0, Some(FakeFailure::Check)),
            (0, Some(FakeFailure::Every)),
        ];
        for (drifts, fails) in failures {
            let (moved, window, _, _) = move_fake(false, drifts, fails);

            assert!(moved.is_err(), "{fails:?}");
            assert!(!window.visible, "{fails:?}");
            assert!(window.shows.is_empty(), "{fails:?}");
        }
    }

    #[test]
    fn failed_show_at_the_target_and_failed_restore_are_both_reported() {
        let (moved, window, _, target) = move_fake(true, 0, Some(FakeFailure::Show));

        assert_eq!(
            moved,
            Err("SetWindowPos failed (Show); showing the pill again also failed: SetWindowPos failed (Show)".to_string())
        );
        assert_eq!(window.placement, target);
        assert!(window.shows.is_empty());
    }

    /// Settles a fake window that Windows moves off its placement on the
    /// next `drifts` checks, recording each commit and each wait.
    fn settle_fake(drifts: usize) -> (Result<Option<u8>, String>, Vec<PlacementVisibility>, usize) {
        let target = placement_at(2000, 144);
        let placement = std::cell::Cell::new(placement_at(0, 96));
        let drifts = std::cell::Cell::new(drifts);
        let mut commits = Vec::new();
        let mut waits = 0;
        let settled = settle_hidden(
            target,
            |next, visibility| {
                placement.set(next);
                commits.push(visibility);
                Ok(())
            },
            |expected| {
                if drifts.get() > 0 {
                    drifts.set(drifts.get() - 1);
                    placement.set(placement_at(2000, 96));
                }
                Ok(placement.get() == expected)
            },
            || waits += 1,
        );
        (settled, commits, waits)
    }

    #[test]
    fn settling_commits_hidden_again_until_two_checks_in_a_row_match() {
        let (settled, commits, waits) = settle_fake(3);

        assert_eq!(settled, Ok(Some(3)));
        assert_eq!(commits, vec![PlacementVisibility::HideWithoutRedraw; 4]);
        assert_eq!(waits, 5);
    }

    #[test]
    fn settling_gives_up_after_twelve_checks_without_showing_the_window() {
        // Each caller turns `None` into its own error and never shows the
        // window: a pill revealed from hidden stays hidden.
        let (settled, commits, waits) = settle_fake(usize::MAX);

        assert_eq!(settled, Ok(None));
        assert_eq!(commits, vec![PlacementVisibility::HideWithoutRedraw; 13]);
        assert_eq!(waits, 12);
    }
}
