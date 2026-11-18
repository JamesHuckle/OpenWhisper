import { describe, expect, it } from "vitest";
import {
  createDictationController,
  type AudioCapture,
  type WidgetState,
  type WorkerSessionEvent,
} from "./dictation-controller";
import { DEFAULT_TRANSCRIPTION_MODEL, LIVE_TRANSCRIPTION_MODEL } from "./models";
import { PCM_MIME_TYPE } from "./pcm-capture";

type Call = { command: string; args: unknown; headers?: Record<string, string> };
type Deferred = { resolve: (value: unknown) => void; reject: (error: unknown) => void };

// What the app answers when a test does not hold or fail a command.
const ANSWERS: Record<string, unknown> = {
  recording_begin: { id: "rec-1" },
  worker_start_session: { session_id: "s-1" },
  recording_import: { id: "imported-1" },
  insert_final_text: { outcome: "pasted", reason: null },
  type_to_target: { outcome: "typed", reason: null },
};

/**
 * A fake Tauri invoke that records every call. A held command stays pending
 * until the test answers it, so calls can be resolved in any order.
 */
function fakeInvoke() {
  const calls: Call[] = [];
  const held = new Map<string, Deferred[]>();
  const holding = new Set<string>();
  const failing = new Map<string, unknown>();
  const answers = { ...ANSWERS };

  const invoke = (command: string, args?: unknown, options?: { headers?: unknown }) => {
    calls.push({ command, args, headers: options?.headers as Record<string, string> | undefined });
    if (failing.has(command)) return Promise.reject(failing.get(command));
    if (holding.has(command)) {
      return new Promise((resolve, reject) => {
        held.set(command, [...(held.get(command) ?? []), { resolve, reject }]);
      });
    }
    return Promise.resolve(answers[command]);
  };

  return {
    invoke,
    calls,
    answers,
    hold: (command: string) => holding.add(command),
    fail: (command: string, error: unknown) => failing.set(command, error),
    /** Answers the oldest held call of a command. */
    release(command: string, value?: unknown) {
      const waiting = held.get(command)?.shift();
      if (!waiting) throw new Error(`${command} is not waiting`);
      waiting.resolve(value ?? answers[command]);
    },
    named: (command: string) => calls.filter((call) => call.command === command),
    commands: () => calls.map((call) => call.command),
  };
}

function fakeCapture() {
  let onAudio: ((chunk: ArrayBuffer) => void) | null = null;
  let active = false;
  const capture: AudioCapture = {
    start: async (send) => {
      onAudio = send;
      active = true;
    },
    isActive: () => active,
    stop: async () => {
      active = false;
    },
    discard: () => {
      active = false;
    },
  };
  return {
    capture,
    /** Sends captured PCM bytes, all set to value. */
    hear(bytes: number, value = 1) {
      if (!onAudio) throw new Error("capture has not started");
      onAudio(new Uint8Array(bytes).fill(value).buffer);
    },
  };
}

// A batch this size is sent at once; a smaller one waits for more audio.
const BATCH = 16 * 1024;

function setup(model = DEFAULT_TRANSCRIPTION_MODEL) {
  const app = fakeInvoke();
  const mic = fakeCapture();
  const states: WidgetState[] = [];
  const toasts: string[] = [];
  const failures: { message: string; recordingSaved: boolean }[] = [];
  const remembered: string[] = [];
  const dictation = createDictationController({
    invoke: app.invoke,
    capture: mic.capture,
    settings: async () => ({ hasOpenaiApiKey: true, transcriptionModel: model }),
    apiKeyKnownMissing: () => false,
    setState: (next) => states.push(next),
    showToast: (message) => toasts.push(message),
    showFailure: (message, recordingSaved) => failures.push({ message, recordingSaved }),
    clearNotice: () => undefined,
    startMeter: () => undefined,
    stopMeter: () => undefined,
    showTranscript: () => undefined,
    clearTranscript: () => undefined,
    rememberTranscript: (transcript) => remembered.push(transcript),
    recordingsChanged: () => undefined,
    closeMenu: () => undefined,
    log: () => undefined,
  });
  return { app, mic, dictation, states, toasts, failures, remembered };
}

async function settle() {
  for (let i = 0; i < 5; i++) await new Promise((resolve) => setTimeout(resolve, 0));
}

function sessionEvent(event: WorkerSessionEvent["event"], done = false): WorkerSessionEvent {
  return { session_id: "s-1", event, done };
}

async function startRecording(dictation: ReturnType<typeof setup>["dictation"]) {
  await dictation.toggleRecording();
  expect(dictation.state).toBe("recording");
}

describe("dictation controller", () => {
  it("shows the listening pill before anything is awaited", () => {
    const { app, dictation, states } = setup();

    void dictation.toggleRecording();

    expect(states).toEqual(["starting"]);
    expect(app.calls).toEqual([]);
  });

  it("starts capture before the backup opens, and the session only after it has", async () => {
    const { app, dictation } = setup();
    app.hold("recording_begin");

    void dictation.toggleRecording();
    await settle();

    expect(dictation.state).toBe("recording");
    expect(app.commands()).toEqual(["capture_paste_target", "recording_begin"]);
    app.release("recording_begin");
    await settle();
    expect(app.commands()).toContain("worker_start_session");
  });

  it("sends Standard audio only to the backup and transcribes the saved file once", async () => {
    const { app, mic, dictation, states } = setup();
    await startRecording(dictation);

    mic.hear(BATCH, 1);
    mic.hear(100, 2);
    await dictation.toggleRecording();
    await settle();

    expect(app.named("recording_transcribe")).toEqual([
      { command: "recording_transcribe", args: { recordingId: "rec-1", sessionId: "s-1" } },
    ]);
    expect(app.named("worker_append_audio_chunk")).toEqual([]);
    expect(app.named("worker_finalize_session_audio")).toEqual([]);
    // Audio goes as raw bytes, with the recording named in a header.
    const backup = app.named("recording_append_chunk");
    expect(backup.map((call) => call.headers)).toEqual([
      { "recording-id": "rec-1" },
      { "recording-id": "rec-1" },
    ]);
    expect(backup.map((call) => (call.args as Uint8Array).byteLength)).toEqual([BATCH, 100]);
    expect(states.at(-1)).toBe("transcribing");
  });

  it("sends every Live batch to the session as it is recorded, then finalizes as PCM", async () => {
    const { app, mic, dictation } = setup(LIVE_TRANSCRIPTION_MODEL);
    await startRecording(dictation);

    mic.hear(BATCH, 1);
    mic.hear(BATCH, 2);
    await settle();

    const live = app.named("worker_append_audio_chunk");
    expect(live.map((call) => call.headers)).toEqual([
      { "session-id": "s-1" },
      { "session-id": "s-1" },
    ]);
    expect(live.map((call) => new Uint8Array(call.args as ArrayBuffer)[0])).toEqual([1, 2]);
    expect(app.named("recording_append_chunk")).toHaveLength(2);

    await dictation.toggleRecording();
    await settle();
    expect(app.named("worker_finalize_session_audio")).toEqual([
      {
        command: "worker_finalize_session_audio",
        args: { sessionId: "s-1", mimeType: PCM_MIME_TYPE },
      },
    ]);
    expect(app.named("recording_transcribe")).toEqual([]);
  });

  it("types Live pieces into the field until the app stops typing", async () => {
    const { app, dictation } = setup(LIVE_TRANSCRIPTION_MODEL);
    await startRecording(dictation);

    app.answers.type_to_target = { outcome: "stopped", reason: "window_changed" };
    await dictation.receiveSessionEvent(sessionEvent({ type: "partial", text: "Hello" }));
    await dictation.receiveSessionEvent(sessionEvent({ type: "partial", text: " there" }));
    await settle();

    expect(app.named("type_to_target").map((call) => call.args)).toEqual([{ text: "Hello" }]);
  });

  it("saves the audio and marks it cancelled when cancelled while recording_begin is pending", async () => {
    const { app, mic, dictation, toasts } = setup();
    app.hold("recording_begin");
    void dictation.toggleRecording();
    await settle();
    mic.hear(100, 7);

    await dictation.cancelTranscription();
    expect(dictation.state).toBe("idle");
    expect(toasts).toContain("Transcription cancelled. Raw audio is still saved.");

    app.release("recording_begin");
    await settle();
    const saved = app.named("recording_append_chunk");
    expect(saved).toHaveLength(1);
    expect(saved[0].headers).toEqual({ "recording-id": "rec-1" });
    expect(Array.from(saved[0].args as Uint8Array)).toEqual(Array(100).fill(7));
    expect(app.named("recording_finish").map((call) => call.args)).toEqual([
      { recordingId: "rec-1", status: "cancelled", transcript: "", error: "Cancelled by user" },
    ]);
    expect(app.commands()).not.toContain("worker_start_session");
  });

  it("stops the session that was starting when cancelled while worker_start_session is pending", async () => {
    const { app, dictation } = setup();
    app.hold("worker_start_session");
    void dictation.toggleRecording();
    await settle();
    expect(app.commands()).toContain("worker_start_session");

    await dictation.cancelTranscription();
    expect(app.named("recording_finish").map((call) => call.args)).toEqual([
      { recordingId: "rec-1", status: "cancelled", transcript: "", error: "Cancelled by user" },
    ]);

    app.release("worker_start_session");
    await settle();
    expect(app.named("worker_stop_session").map((call) => call.args)).toEqual([
      { sessionId: "s-1" },
    ]);
    expect(app.commands()).not.toContain("recording_transcribe");
    expect(dictation.state).toBe("idle");
  });

  it("shows a session error while recording as a failure and keeps the recording", async () => {
    const { app, mic, dictation, failures, toasts } = setup();
    await startRecording(dictation);
    mic.hear(100);

    await dictation.receiveSessionEvent(
      sessionEvent({ type: "error", text: "OpenAI is unreachable" }, true),
    );

    expect(failures).toEqual([{ message: "OpenAI is unreachable", recordingSaved: true }]);
    expect(toasts).toEqual([]);
    expect(app.named("recording_append_chunk")).toHaveLength(1);
    expect(app.named("recording_finish").map((call) => call.args)).toEqual([
      { recordingId: "rec-1", status: "failed", transcript: "", error: "OpenAI is unreachable" },
    ]);
    expect(dictation.lastFailedRecordingId).toBe("rec-1");
    expect(dictation.state).toBe("idle");
  });

  it("pastes a dictation's final transcript through insert_final_text", async () => {
    const { app, dictation, remembered } = setup();
    await startRecording(dictation);
    await dictation.toggleRecording();

    await dictation.receiveSessionEvent(sessionEvent({ type: "final", text: "Hello world" }, true));

    expect(app.named("insert_final_text").map((call) => call.args)).toEqual([
      { text: "Hello world" },
    ]);
    expect(app.commands()).not.toContain("copy_to_clipboard");
    expect(app.named("recording_finish").map((call) => call.args)).toEqual([
      { recordingId: "rec-1", status: "complete", transcript: "Hello world", error: "" },
    ]);
    expect(remembered).toEqual(["Hello world"]);
    expect(dictation.state).toBe("idle");
  });

  it("copies the final transcript of an imported file instead of pasting it", async () => {
    const { app, dictation } = setup();

    await dictation.transcribeImportedFile();
    expect(app.named("worker_start_session").map((call) => call.args)).toEqual([
      { profileId: "file-import" },
    ]);
    expect(app.named("recording_transcribe").map((call) => call.args)).toEqual([
      { recordingId: "imported-1", sessionId: "s-1" },
    ]);

    await dictation.receiveSessionEvent(sessionEvent({ type: "final", text: "From a file" }, true));
    expect(app.named("copy_to_clipboard").map((call) => call.args)).toEqual([
      { text: "From a file" },
    ]);
    expect(app.commands()).not.toContain("insert_final_text");
  });

  it("retranscribes a saved recording from disk and copies its transcript", async () => {
    const { app, dictation } = setup();

    await dictation.retranscribeRecording("rec-9");
    expect(app.named("recording_finish")[0].args).toEqual({
      recordingId: "rec-9",
      status: "transcribing",
      transcript: "",
      error: "",
    });
    expect(app.named("worker_start_session").map((call) => call.args)).toEqual([
      { profileId: "recording-retry" },
    ]);
    expect(app.named("recording_transcribe").map((call) => call.args)).toEqual([
      { recordingId: "rec-9", sessionId: "s-1" },
    ]);

    await dictation.receiveSessionEvent(sessionEvent({ type: "final", text: "Again" }, true));
    expect(app.named("copy_to_clipboard").map((call) => call.args)).toEqual([{ text: "Again" }]);
    expect(app.commands()).not.toContain("insert_final_text");
  });

  it("fails the dictation when the backup could not be written", async () => {
    const { app, mic, dictation, failures } = setup();
    app.fail("recording_append_chunk", new Error("Disk full"));
    await startRecording(dictation);
    mic.hear(BATCH);

    await dictation.toggleRecording();
    await settle();

    expect(failures).toEqual([{ message: "Disk full", recordingSaved: true }]);
    expect(app.commands()).not.toContain("recording_transcribe");
    expect(app.named("recording_finish").map((call) => call.args)).toEqual([
      { recordingId: "rec-1", status: "failed", transcript: "", error: "Disk full" },
    ]);
    expect(dictation.state).toBe("idle");
  });

  it("stops once the microphone is open after a second toggle press during startup", async () => {
    const { app, dictation } = setup();
    app.hold("recording_begin");

    const first = dictation.toggleRecording();
    await dictation.toggleRecording();
    await settle();
    expect(dictation.state).toBe("transcribing");

    app.release("recording_begin");
    await first;
    await settle();
    expect(app.named("recording_transcribe").map((call) => call.args)).toEqual([
      { recordingId: "rec-1", sessionId: "s-1" },
    ]);
  });

  it("stops at once when the hold shortcut is released while the microphone opens", async () => {
    const { app, dictation } = setup();
    app.hold("recording_begin");

    const press = dictation.pressToTalkStart();
    await dictation.pressToTalkStop();
    await settle();
    expect(dictation.state).toBe("transcribing");

    app.release("recording_begin");
    await press;
    await settle();
    expect(app.named("recording_transcribe")).toHaveLength(1);
  });

  it("never pastes a transcript that arrives while a cancel stops its session", async () => {
    const { app, dictation } = setup();
    await startRecording(dictation);
    await dictation.toggleRecording();
    app.hold("worker_stop_session");

    const cancelling = dictation.cancelTranscription();
    await dictation.receiveSessionEvent(sessionEvent({ type: "final", text: "Too late" }, true));
    app.release("worker_stop_session");
    await cancelling;
    await settle();

    expect(app.commands()).not.toContain("insert_final_text");
    expect(app.commands()).not.toContain("copy_to_clipboard");
    expect(app.named("recording_finish").map((call) => call.args)).toEqual([
      { recordingId: "rec-1", status: "cancelled", transcript: "", error: "Cancelled by user" },
    ]);
    expect(dictation.state).toBe("idle");
  });
});
