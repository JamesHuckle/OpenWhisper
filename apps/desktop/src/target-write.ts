import type { Invoke } from "./recording-pipeline";

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

/** What delivering a finished transcript needs from the page. */
export type FinalTranscriptDelivery = {
  invoke: Invoke;
  transcript: string;
  /** Paste into the locked field; otherwise copy to the clipboard. */
  paste: boolean;
  /** False once a cancel or a new recording takes the session over. */
  owned: () => boolean;
  /** Saves the transcript with the recording, so the history card keeps it. */
  saveRecording: (transcript: string) => Promise<void>;
  /** Keeps the transcript as the last one (memory and localStorage). */
  rememberLocally: (transcript: string) => void;
  /** Resolves once live typing into the field has finished. */
  liveTypingDone: () => Promise<unknown>;
  notify: (message: string, durationMs: number) => void;
  log: (message: string) => void;
};

/**
 * Saves a finished transcript first, then copies or pastes it. A transcript
 * the service returned is never thrown away because a later step failed: the
 * user is told it could not be pasted and can copy it from the history card,
 * without paying to transcribe the recording again.
 */
export async function deliverFinalTranscript(steps: FinalTranscriptDelivery): Promise<void> {
  const { invoke, transcript, owned, notify, log } = steps;
  if (!owned()) return;
  await steps.saveRecording(transcript);
  try {
    steps.rememberLocally(transcript);
  } catch (error) {
    log(`last transcript not stored=${describe(error)}`);
  }
  await steps.liveTypingDone();
  if (!owned()) return;
  try {
    if (!steps.paste) {
      await invoke("copy_to_clipboard", { text: transcript });
      notify("Transcript copied to clipboard", 3000);
    } else {
      const write = (await invoke("insert_final_text", { text: transcript })) as TargetWrite;
      notify(finalWriteNotice(write), write.outcome === "copied" ? 5000 : 2000);
    }
  } catch (error) {
    log(`final transcript delivery failed=${describe(error)}`);
    notify(
      steps.paste
        ? "Transcript saved; could not paste it. Copy it from the menu"
        : "Transcript saved; could not copy it. Copy it from the menu",
      6000,
    );
  }
}

function describe(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
