import { describe, expect, it } from "vitest";
import {
  deliverFinalTranscript,
  finalWriteNotice,
  type FinalTranscriptDelivery,
} from "./target-write";

describe("final transcript notices", () => {
  it("confirms text written into the target field", () => {
    expect(finalWriteNotice({ outcome: "replaced", reason: null })).toBe("Transcribed live");
    expect(finalWriteNotice({ outcome: "kept", reason: null })).toBe("Transcribed live");
    expect(finalWriteNotice({ outcome: "pasted", reason: null })).toBe("Transcribed and pasted");
  });

  it("says why the transcript was copied instead of typed", () => {
    expect(finalWriteNotice({ outcome: "copied", reason: "password_field" })).toBe(
      "Not typed into a password field. Transcript copied",
    );
    expect(finalWriteNotice({ outcome: "copied", reason: "window_changed" })).toBe(
      "You switched windows, so the transcript was copied",
    );
    expect(finalWriteNotice({ outcome: "copied", reason: "no_target" })).toBe(
      "No text field to paste into. Transcript copied",
    );
    expect(finalWriteNotice({ outcome: "copied", reason: "unsupported" })).toBe(
      "Transcript copied to clipboard",
    );
  });
});

// Runs the delivery the way dictation-controller.ts does, over a fake Tauri invoke that
// records every command and can make one of them fail.
function delivery(options: {
  paste?: boolean;
  fail?: string;
  saveFails?: boolean;
  storageThrows?: boolean;
  cancelWhileTyping?: boolean;
}) {
  const commands: string[] = [];
  const saved: { status: string; transcript: string }[] = [];
  const notices: string[] = [];
  let owner = "session-1";
  let remembered = "";
  const steps: FinalTranscriptDelivery = {
    invoke: async (command) => {
      commands.push(command);
      if (command === options.fail) throw new Error(`${command} failed`);
      return command === "insert_final_text" ? { outcome: "pasted", reason: null } : null;
    },
    transcript: "Hello world",
    paste: options.paste ?? true,
    owned: () => owner === "session-1",
    saveRecording: async (transcript) => {
      if (options.saveFails) return false;
      saved.push({ status: "complete", transcript });
      return true;
    },
    rememberLocally: (transcript) => {
      remembered = transcript;
      if (options.storageThrows) throw new Error("QuotaExceededError");
    },
    liveTypingDone: async () => {
      if (options.cancelWhileTyping) owner = "cancelled";
    },
    notify: (message) => notices.push(message),
    log: () => {},
  };
  return { steps, commands, saved, notices, remembered: () => remembered };
}

// No step may ask the worker or the service for a new transcription.
function expectNoNewTranscription(commands: string[]) {
  const deliveryCommands = ["copy_to_clipboard", "insert_final_text"];
  expect(commands.filter((command) => !deliveryCommands.includes(command))).toEqual([]);
}

describe("delivering the final transcript", () => {
  it("saves the transcript before pasting it", async () => {
    const run = delivery({});
    await deliverFinalTranscript(run.steps);
    expect(run.saved).toEqual([{ status: "complete", transcript: "Hello world" }]);
    expect(run.commands).toEqual(["insert_final_text"]);
    expect(run.notices).toEqual(["Transcribed and pasted"]);
  });

  it("keeps the transcript when the clipboard copy fails", async () => {
    const run = delivery({ paste: false, fail: "copy_to_clipboard" });
    await expect(deliverFinalTranscript(run.steps)).resolves.toBeUndefined();
    expect(run.saved).toEqual([{ status: "complete", transcript: "Hello world" }]);
    expect(run.remembered()).toBe("Hello world");
    expect(run.notices).toEqual(["Transcript saved; could not copy it. Copy it from the menu"]);
    expectNoNewTranscription(run.commands);
  });

  it("keeps the transcript when pasting into the field fails", async () => {
    const run = delivery({ fail: "insert_final_text" });
    await expect(deliverFinalTranscript(run.steps)).resolves.toBeUndefined();
    expect(run.saved).toEqual([{ status: "complete", transcript: "Hello world" }]);
    expect(run.notices).toEqual(["Transcript saved; could not paste it. Copy it from the menu"]);
    expectNoNewTranscription(run.commands);
  });

  it("never says the transcript was saved when saving it and pasting it both fail", async () => {
    const run = delivery({ fail: "insert_final_text", saveFails: true });
    await expect(deliverFinalTranscript(run.steps)).resolves.toBeUndefined();
    expect(run.remembered()).toBe("Hello world");
    expect(run.notices).toEqual(["Transcript not saved; could not paste it. Transcribe it again from the menu"]);
    expectNoNewTranscription(run.commands);
  });

  it("still saves and pastes when localStorage throws", async () => {
    const run = delivery({ storageThrows: true });
    await expect(deliverFinalTranscript(run.steps)).resolves.toBeUndefined();
    expect(run.saved).toEqual([{ status: "complete", transcript: "Hello world" }]);
    expect(run.commands).toEqual(["insert_final_text"]);
    expectNoNewTranscription(run.commands);
  });

  it("keeps the saved transcript but does not paste after a cancel during live typing", async () => {
    const run = delivery({ cancelWhileTyping: true });
    await deliverFinalTranscript(run.steps);
    expect(run.saved).toEqual([{ status: "complete", transcript: "Hello world" }]);
    expect(run.commands).toEqual([]);
    expect(run.notices).toEqual([]);
  });
});
