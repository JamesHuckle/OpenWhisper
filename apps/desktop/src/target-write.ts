/**
 * Result of writing dictated text into the field that was focused when the
 * recording started (the native `type_to_target` and `insert_final_text`
 * commands). The native side checks that field before every write.
 */
export type TargetWrite = {
  outcome: "typed" | "stopped" | "replaced" | "pasted" | "kept" | "copied";
  reason: string | null;
};

/** The short notice shown after the final transcript is written. */
export function finalWriteNotice(write: TargetWrite): string {
  switch (write.outcome) {
    case "replaced":
    case "kept":
      return "Transcribed live";
    case "pasted":
      return "Transcribed and pasted";
    default:
      switch (write.reason) {
        case "password_field":
          return "Not typed into a password field. Transcript copied";
        case "window_changed":
          return "You switched windows, so the transcript was copied";
        case "no_target":
          return "No text field to paste into. Transcript copied";
        case "shortcut_held":
          return "A shortcut key was held, so the transcript was copied";
        default:
          return "Transcript copied to clipboard";
      }
  }
}
