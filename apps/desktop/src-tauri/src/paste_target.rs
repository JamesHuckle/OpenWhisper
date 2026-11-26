//! Decides whether dictated text may be written into the field that was
//! focused when recording started.
//!
//! The rules are plain data in, decision out, so they are shared by every
//! platform and tested without Windows. The Windows code in `lib.rs` gathers
//! an [`Observation`] just before each write and does what the decision says.
//! Its only dependency is Serde, which sends each [`WriteOutcome`] to the
//! frontend.

/// What the app saw just before writing, after it tried to bring the
/// recording's target window back to the front.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation {
    /// The window captured when recording started still exists.
    pub target_alive: bool,
    /// That window now has keyboard focus.
    pub foreground_is_target: bool,
    /// `Some(false)` when the focused control is known to differ from the
    /// one captured when recording started; `None` when it is unknown.
    pub same_field: Option<bool>,
    /// The focused control hides what is typed (a password field).
    pub is_password: bool,
    /// The user pressed a key or clicked outside OpenWhisper since the last
    /// write, so the cursor may have moved.
    pub user_input_since_write: bool,
    /// Alt or the Windows key is held, so typed characters or Backspace
    /// could act as shortcuts.
    pub shortcut_modifier_held: bool,
}

/// Why text was not written into the target field. Sent to the frontend as
/// the `reason` of a [`WriteOutcome`], in snake case (`window_changed`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Blocked {
    NoTarget,
    WindowChanged,
    PasswordField,
    FieldChanged,
    CursorMoved,
    ShortcutHeld,
    /// Windows did not accept every injected key (for example an elevated
    /// window or a locked desktop), so what reached the field is unknown.
    InputRefused,
    /// This system cannot write into other apps' fields (only Windows can).
    Unsupported,
}

/// How a write into the recording's target field ended, as the frontend
/// receives it: `{"outcome":"stopped","reason":"window_changed"}`, or just
/// `{"outcome":"typed"}` when text was written. `stopped` ends live typing for
/// this recording; `kept` means the live draft already matched; `copied` left
/// the transcript on the clipboard instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "outcome", content = "reason", rename_all = "snake_case")]
pub enum WriteOutcome {
    Typed,
    Replaced,
    Pasted,
    Kept,
    Stopped(Blocked),
    Copied(Blocked),
}

/// What to do with one live piece of the transcript.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveWrite {
    Type,
    /// Stop live typing for the rest of this recording.
    Stop(Blocked),
}

/// What to do with the final transcript.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinalWrite {
    /// Delete the live draft with this many Backspaces, then paste.
    Replace { delete_chars: usize },
    /// Paste without deleting anything.
    Paste,
    /// The live draft already says the final text.
    KeepDraft,
    /// Do not touch the target; put the transcript on the clipboard.
    Copy(Blocked),
}

fn blocked_for_target(obs: &Observation) -> Option<Blocked> {
    if !obs.target_alive {
        Some(Blocked::NoTarget)
    } else if !obs.foreground_is_target {
        Some(Blocked::WindowChanged)
    } else if obs.is_password {
        Some(Blocked::PasswordField)
    } else if obs.shortcut_modifier_held {
        Some(Blocked::ShortcutHeld)
    } else {
        None
    }
}

/// Live pieces are typed only while nothing has disturbed the draft: the same
/// field still has focus and, once text has been typed, the user has not
/// pressed a key or clicked (which could move the cursor).
pub fn decide_live(obs: &Observation, draft_written: bool) -> LiveWrite {
    if let Some(blocked) = blocked_for_target(obs) {
        return LiveWrite::Stop(blocked);
    }
    if obs.same_field == Some(false) {
        return LiveWrite::Stop(Blocked::FieldChanged);
    }
    if draft_written && obs.user_input_since_write {
        return LiveWrite::Stop(Blocked::CursorMoved);
    }
    LiveWrite::Type
}

/// The live draft is deleted only when it is provably still where it was
/// typed: live typing never stopped, the field is the same and the user has
/// not pressed a key or clicked since the last piece. Otherwise the final
/// text is pasted after it and nothing is deleted.
pub fn decide_final(
    obs: &Observation,
    draft: &str,
    draft_intact: bool,
    final_text: &str,
) -> FinalWrite {
    if let Some(blocked) = blocked_for_target(obs) {
        return FinalWrite::Copy(blocked);
    }
    if draft.is_empty() {
        return FinalWrite::Paste;
    }
    let undisturbed = draft_intact && obs.same_field != Some(false) && !obs.user_input_since_write;
    if !undisturbed {
        return FinalWrite::Paste;
    }
    if draft.trim() == final_text.trim() {
        FinalWrite::KeepDraft
    } else {
        FinalWrite::Replace {
            delete_chars: draft.chars().count(),
        }
    }
}

/// Live pieces are typed as keystrokes. A typed line break or tab would
/// press Enter or Tab (sending a chat message or leaving the field), so
/// control characters are typed as spaces.
pub fn typeable_text(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steady() -> Observation {
        Observation {
            target_alive: true,
            foreground_is_target: true,
            same_field: Some(true),
            is_password: false,
            user_input_since_write: false,
            shortcut_modifier_held: false,
        }
    }

    #[test]
    fn live_pieces_type_into_the_unchanged_field() {
        assert_eq!(decide_live(&steady(), false), LiveWrite::Type);
        assert_eq!(decide_live(&steady(), true), LiveWrite::Type);
        let unknown_field = Observation {
            same_field: None,
            ..steady()
        };
        assert_eq!(decide_live(&unknown_field, true), LiveWrite::Type);
    }

    #[test]
    fn live_typing_stops_when_the_user_switches_window_or_field() {
        let other_window = Observation {
            foreground_is_target: false,
            ..steady()
        };
        assert_eq!(
            decide_live(&other_window, true),
            LiveWrite::Stop(Blocked::WindowChanged)
        );
        let closed = Observation {
            target_alive: false,
            ..steady()
        };
        assert_eq!(
            decide_live(&closed, false),
            LiveWrite::Stop(Blocked::NoTarget)
        );
        let other_field = Observation {
            same_field: Some(false),
            ..steady()
        };
        assert_eq!(
            decide_live(&other_field, false),
            LiveWrite::Stop(Blocked::FieldChanged)
        );
    }

    #[test]
    fn live_typing_stops_after_the_user_moves_the_cursor() {
        let moved = Observation {
            user_input_since_write: true,
            ..steady()
        };
        assert_eq!(
            decide_live(&moved, true),
            LiveWrite::Stop(Blocked::CursorMoved)
        );
        // Before the first piece there is no draft to protect: a click into
        // the field after starting the recording is fine.
        assert_eq!(decide_live(&moved, false), LiveWrite::Type);
    }

    #[test]
    fn nothing_is_typed_into_password_fields() {
        let password = Observation {
            is_password: true,
            ..steady()
        };
        assert_eq!(
            decide_live(&password, false),
            LiveWrite::Stop(Blocked::PasswordField)
        );
        assert_eq!(
            decide_final(&password, "", true, "secret words"),
            FinalWrite::Copy(Blocked::PasswordField)
        );
    }

    #[test]
    fn held_alt_or_windows_key_blocks_keystrokes() {
        let held = Observation {
            shortcut_modifier_held: true,
            ..steady()
        };
        assert_eq!(
            decide_live(&held, false),
            LiveWrite::Stop(Blocked::ShortcutHeld)
        );
        assert_eq!(
            decide_final(&held, "draft", true, "final"),
            FinalWrite::Copy(Blocked::ShortcutHeld)
        );
    }

    #[test]
    fn final_text_replaces_an_undisturbed_draft() {
        assert_eq!(
            decide_final(&steady(), "helo wörld", true, "Hello world."),
            FinalWrite::Replace { delete_chars: 10 }
        );
        assert_eq!(
            decide_final(&steady(), "Hello world. ", true, "Hello world."),
            FinalWrite::KeepDraft
        );
    }

    #[test]
    fn a_disturbed_draft_is_never_deleted() {
        let moved = Observation {
            user_input_since_write: true,
            ..steady()
        };
        assert_eq!(
            decide_final(&moved, "draft", true, "final"),
            FinalWrite::Paste
        );
        let other_field = Observation {
            same_field: Some(false),
            ..steady()
        };
        assert_eq!(
            decide_final(&other_field, "draft", true, "final"),
            FinalWrite::Paste
        );
        assert_eq!(
            decide_final(&steady(), "draft", false, "final"),
            FinalWrite::Paste
        );
    }

    #[test]
    fn final_text_is_copied_when_the_target_window_is_gone_or_left() {
        let other_window = Observation {
            foreground_is_target: false,
            ..steady()
        };
        assert_eq!(
            decide_final(&other_window, "draft", true, "final"),
            FinalWrite::Copy(Blocked::WindowChanged)
        );
        let closed = Observation {
            target_alive: false,
            ..steady()
        };
        assert_eq!(
            decide_final(&closed, "", true, "final"),
            FinalWrite::Copy(Blocked::NoTarget)
        );
    }

    #[test]
    fn final_text_without_a_draft_is_pasted() {
        assert_eq!(
            decide_final(&steady(), "", true, "Hello."),
            FinalWrite::Paste
        );
        let moved = Observation {
            user_input_since_write: true,
            ..steady()
        };
        assert_eq!(decide_final(&moved, "", true, "Hello."), FinalWrite::Paste);
    }

    #[test]
    fn write_outcomes_reach_the_frontend_as_outcome_and_reason() {
        let json = |outcome: WriteOutcome| serde_json::to_string(&outcome).unwrap();
        assert_eq!(
            json(WriteOutcome::Stopped(Blocked::WindowChanged)),
            r#"{"outcome":"stopped","reason":"window_changed"}"#
        );
        assert_eq!(
            json(WriteOutcome::Copied(Blocked::PasswordField)),
            r#"{"outcome":"copied","reason":"password_field"}"#
        );
        assert_eq!(
            json(WriteOutcome::Copied(Blocked::Unsupported)),
            r#"{"outcome":"copied","reason":"unsupported"}"#
        );
        assert_eq!(json(WriteOutcome::Typed), r#"{"outcome":"typed"}"#);
        assert_eq!(json(WriteOutcome::Kept), r#"{"outcome":"kept"}"#);
    }

    #[test]
    fn typed_pieces_never_press_enter_or_tab() {
        assert_eq!(typeable_text("one\ntwo\r\n\tthree"), "one two   three");
        assert_eq!(typeable_text("café 👍"), "café 👍");
    }
}
