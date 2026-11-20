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
    /** Fails the oldest held call of a command. */
    reject(command: string, error: unknown) {
      const waiting = held.get(command)?.shift();
      if (!waiting) throw new Error(`${command} is not waiting`);
      waiting.reject(error);
    },
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
  let stopHeld: Promise<void> | null = null;
  let finishStop = () => undefined as void;
  const capture: AudioCapture = {
    start: async (send) => {
      onAudio = send;
      active = true;
    },
    isActive: () => active,
    stop: async () => {
      if (stopHeld) await stopHeld;
      active = false;
    },
    discard: () => {
      active = false;
    },
  };
  return {
    capture,
    /** Keeps stop waiting, as the worklet does for its last samples, until finishStop. */
    holdStop() {
      stopHeld = new Promise((resolve) => {
        finishStop = () => {
          stopHeld = null;
          resolve();
        };
      });
    },
    finishStop: () => finishStop(),
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
  const logs: string[] = [];
  const changes = { recordings: 0 };
  // The model the menu has selected; a test may change it between recordings.
  const config = { model };
  const dictation = createDictationController({
    invoke: app.invoke,
    capture: mic.capture,
    settings: async () => ({ hasOpenaiApiKey: true, transcriptionModel: config.model }),
    apiKeyKnownMissing: () => false,
    setState: (next) => states.push(next),
    showToast: (message) => toasts.push(message),
    showFailure: (message, recordingSaved) => failures.push({ message, recordingSaved }),
    clearNotice: () => undefined,
    startMeter: () => undefined,
    stopMeter: () => undefined,
    showTranscript: () => undefined,
    clearTranscript: () => undefined,
    recordingsChanged: () => {
      changes.recordings += 1;
    },
    closeMenu: () => undefined,
    log: (message) => logs.push(message),
  });
  return { app, mic, dictation, states, toasts, failures, logs, config, changes };
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

  it("logs a cancelled backup whose metadata could not be saved and still lists it", async () => {
    const { app, dictation, logs, changes } = setup();
    app.hold("recording_begin");
    void dictation.toggleRecording();
    await settle();
    await dictation.cancelTranscription();
    const before = changes.recordings;

    app.fail("recording_finish", new Error("Disk full"));
    app.release("recording_begin");
    await settle();

    expect(app.named("recording_finish").map((call) => call.args)).toEqual([
      { recordingId: "rec-1", status: "cancelled", transcript: "", error: "Cancelled by user" },
    ]);
    expect(logs).toContain("recording metadata update failed=Disk full");
    expect(changes.recordings).toBe(before + 1);
  });

  it("fails a dictation whose session could not start and keeps its recording", async () => {
    const { app, mic, dictation, failures, states } = setup();
    app.fail("worker_start_session", new Error("Could not connect"));

    await dictation.toggleRecording();
    await settle();

    expect(failures).toEqual([{ message: "Could not connect", recordingSaved: true }]);
    expect(app.named("recording_finish").map((call) => call.args)).toEqual([
      { recordingId: "rec-1", status: "failed", transcript: "", error: "Could not connect" },
    ]);
    expect(dictation.lastFailedRecordingId).toBe("rec-1");
    expect(mic.capture.isActive()).toBe(false);
    expect(states.at(-1)).toBe("idle");
    expect(dictation.state).toBe("idle");
  });

  it("saves the microphone's last samples when cancelled during a stop while recording_begin is pending", async () => {
    const { app, mic, dictation } = setup();
    app.hold("recording_begin");
    void dictation.toggleRecording();
    await settle();
    mic.hear(100, 7);

    mic.holdStop();
    const stopping = dictation.stopRecording();
    const cancelling = dictation.cancelTranscription();
    app.release("recording_begin");
    await settle();
    mic.hear(50, 8);
    mic.finishStop();
    await Promise.all([stopping, cancelling]);
    await settle();

    const saved = app.named("recording_append_chunk").map((call) => Array.from(call.args as Uint8Array));
    expect(saved.flat()).toEqual([...Array(100).fill(7), ...Array(50).fill(8)]);
    expect(app.named("recording_finish").map((call) => call.args)).toEqual([
      { recordingId: "rec-1", status: "cancelled", transcript: "", error: "Cancelled by user" },
    ]);
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
    const { app, dictation } = setup();
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
    expect(dictation.state).toBe("idle");
  });

  it("copies the final transcript of an imported file instead of pasting it", async () => {
    const { app, dictation } = setup();

    await dictation.transcribeImportedFile();
    expect(app.named("worker_start_session").map((call) => call.args)).toEqual([
      { profileId: "file-import", standardOnly: true },
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
      { profileId: "recording-retry", standardOnly: true },
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
    expect(app.named("worker_stop_session").map((call) => call.args)).toEqual([
      { sessionId: "s-1" },
    ]);
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

  it("logs a session that fails to stop and still finishes the cancel", async () => {
    const { app, dictation, logs } = setup();
    await startRecording(dictation);
    app.fail("worker_stop_session", new Error("Session already gone"));

    await dictation.cancelTranscription();

    expect(app.named("worker_stop_session").map((call) => call.args)).toEqual([{ sessionId: "s-1" }]);
    expect(logs).toContain("stop session failed=Session already gone");
    expect(dictation.state).toBe("idle");
  });

  it("lets a second cancel wait for the first, so it never cancels the next recording", async () => {
    const { app, dictation } = setup();
    await startRecording(dictation);
    await dictation.toggleRecording();
    app.hold("worker_stop_session");

    const first = dictation.cancelTranscription();
    const second = dictation.cancelTranscription();
    await settle();
    app.answers.recording_begin = { id: "rec-2" };
    void dictation.toggleRecording();
    await settle();
    app.release("worker_stop_session");
    await Promise.all([first, second]);
    await settle();

    expect(app.named("recording_finish").map((call) => call.args)).toEqual([
      { recordingId: "rec-1", status: "cancelled", transcript: "", error: "Cancelled by user" },
    ]);
    expect(dictation.state).toBe("idle");
    await startRecording(dictation);
    expect(dictation.currentRecordingId).toBe("rec-2");
  });

  it("drops Live pieces queued for a cancelled session instead of typing them into the next one", async () => {
    const { app, dictation } = setup(LIVE_TRANSCRIPTION_MODEL);
    await startRecording(dictation);
    app.hold("type_to_target");
    await dictation.receiveSessionEvent(sessionEvent({ type: "partial", text: "Old" }));
    await dictation.receiveSessionEvent(sessionEvent({ type: "partial", text: " words" }));
    await settle();
    await dictation.toggleRecording();
    await dictation.cancelTranscription();

    app.answers.worker_start_session = { session_id: "s-2" };
    await startRecording(dictation);
    app.release("type_to_target");
    await settle();

    expect(app.named("type_to_target").map((call) => call.args)).toEqual([{ text: "Old" }]);
  });

  it("leaves the next recording alone when a cancelled startup fails late", async () => {
    const { app, mic, dictation, failures } = setup();
    app.hold("worker_start_session");
    void dictation.toggleRecording();
    await settle();
    await dictation.toggleRecording();
    await dictation.cancelTranscription();

    app.answers.recording_begin = { id: "rec-2" };
    app.answers.worker_start_session = { session_id: "s-2" };
    // The hold shortcut, as the toggle still counts the first startup as running.
    const second = dictation.pressToTalkStart();
    await settle();
    expect(dictation.state).toBe("recording");
    // The cancelled recording's session request fails only now.
    app.reject("worker_start_session", new Error("Could not connect"));
    await settle();

    expect(failures).toEqual([]);
    expect(dictation.state).toBe("recording");
    expect(mic.capture.isActive()).toBe(true);
    expect(app.named("recording_finish").map((call) => call.args)).toEqual([
      { recordingId: "rec-1", status: "cancelled", transcript: "", error: "Cancelled by user" },
    ]);
    app.release("worker_start_session");
    await second;
    await dictation.pressToTalkStop();
    await settle();
    expect(app.named("recording_transcribe").map((call) => call.args)).toEqual([
      { recordingId: "rec-2", sessionId: "s-2" },
    ]);
  });

  it("never types a Standard recording into the field when a cancelled Live start opens late", async () => {
    const { app, dictation, config } = setup(LIVE_TRANSCRIPTION_MODEL);
    app.hold("worker_start_session");
    void dictation.toggleRecording();
    await settle();
    await dictation.cancelTranscription();

    config.model = DEFAULT_TRANSCRIPTION_MODEL;
    app.answers.recording_begin = { id: "rec-2" };
    const second = dictation.pressToTalkStart();
    await settle();
    // The cancelled Live start opens only now, while the Standard one is starting.
    app.release("worker_start_session", { session_id: "s-1" });
    await settle();
    app.release("worker_start_session", { session_id: "s-2" });
    await second;
    await settle();
    expect(dictation.state).toBe("recording");

    await dictation.receiveSessionEvent({ session_id: "s-2", event: { type: "partial", text: "Hi" } });
    await settle();
    expect(app.commands()).not.toContain("type_to_target");
  });

  it("finalizes once, after the last samples are saved, when the session opens during a stop", async () => {
    const { app, mic, dictation } = setup();
    app.hold("worker_start_session");
    void dictation.toggleRecording();
    await settle();
    expect(dictation.state).toBe("recording");

    mic.holdStop();
    const stopping = dictation.stopRecording();
    app.release("worker_start_session");
    await settle();
    mic.hear(100, 9);
    mic.finishStop();
    await stopping;
    await settle();

    const commands = app.commands();
    expect(app.named("recording_transcribe")).toHaveLength(1);
    expect(commands.lastIndexOf("recording_append_chunk")).toBeLessThan(
      commands.indexOf("recording_transcribe"),
    );
    expect(Array.from(app.named("recording_append_chunk").at(-1)!.args as Uint8Array)).toEqual(
      Array(100).fill(9),
    );
  });

  it("saves the microphone's last samples when cancelled while a stop waits for them", async () => {
    const { app, mic, dictation } = setup();
    await startRecording(dictation);
    mic.hear(100, 3);

    mic.holdStop();
    const stopping = dictation.stopRecording();
    await settle();
    const cancelling = dictation.cancelTranscription();
    mic.hear(50, 4);
    mic.finishStop();
    await Promise.all([stopping, cancelling]);
    await settle();

    const saved = app.named("recording_append_chunk").map((call) => Array.from(call.args as Uint8Array));
    expect(saved.flat()).toEqual([...Array(100).fill(3), ...Array(50).fill(4)]);
    const commands = app.commands();
    expect(commands.lastIndexOf("recording_append_chunk")).toBeLessThan(
      commands.indexOf("recording_finish"),
    );
    expect(app.named("recording_finish").map((call) => call.args)).toEqual([
      { recordingId: "rec-1", status: "cancelled", transcript: "", error: "Cancelled by user" },
    ]);
    expect(app.commands()).not.toContain("recording_transcribe");
  });

  it("stops the Live session when its audio could not be sent and transcribes the saved recording with Standard", async () => {
    const { app, mic, dictation, failures } = setup(LIVE_TRANSCRIPTION_MODEL);
    app.fail("worker_append_audio_chunk", new Error("Connection lost"));
    await startRecording(dictation);
    mic.hear(BATCH);
    app.answers.worker_start_session = { session_id: "s-2" };

    await dictation.toggleRecording();
    await settle();

    expect(app.commands()).not.toContain("worker_finalize_session_audio");
    expect(app.named("worker_stop_session").map((call) => call.args)).toEqual([
      { sessionId: "s-1" },
    ]);
    expect(app.named("recording_transcribe").map((call) => call.args)).toEqual([
      { recordingId: "rec-1", sessionId: "s-2" },
    ]);
    expect(failures).toEqual([]);
    expect(dictation.state).toBe("transcribing");
  });

  describe("a failed Live dictation", () => {
    async function liveDictationStopped() {
      const env = setup(LIVE_TRANSCRIPTION_MODEL);
      await startRecording(env.dictation);
      env.mic.hear(BATCH);
      await settle();
      env.app.answers.worker_start_session = { session_id: "s-2" };
      return env;
    }

    async function expectStandardTranscriptPasted(env: Awaited<ReturnType<typeof liveDictationStopped>>) {
      const { app, dictation, failures } = env;
      expect(app.named("worker_start_session").map((call) => call.args)).toEqual([
        { profileId: "default" },
        { profileId: "default", standardOnly: true },
      ]);
      expect(app.named("recording_transcribe").map((call) => call.args)).toEqual([
        { recordingId: "rec-1", sessionId: "s-2" },
      ]);
      await dictation.receiveSessionEvent({
        session_id: "s-2",
        event: { type: "final", text: "Hello from Standard" },
        done: true,
      });
      // Pasted, so it replaces the Live draft in the field.
      expect(app.named("insert_final_text").map((call) => call.args)).toEqual([
        { text: "Hello from Standard" },
      ]);
      expect(app.named("recording_finish").map((call) => call.args)).toEqual([
        { recordingId: "rec-1", status: "complete", transcript: "Hello from Standard", error: "" },
      ]);
        expect(failures).toEqual([]);
      expect(dictation.state).toBe("idle");
    }

    it("is transcribed with Standard when finalizing it fails", async () => {
      const env = await liveDictationStopped();
      env.app.fail("worker_finalize_session_audio", new Error("Session is already finalizing"));

      await env.dictation.toggleRecording();
      await settle();

      expect(env.app.named("worker_stop_session").map((call) => call.args)).toEqual([
        { sessionId: "s-1" },
      ]);
      await expectStandardTranscriptPasted(env);
    });

    it("stops the Standard session when transcribing the saved recording fails", async () => {
      const env = await liveDictationStopped();
      env.app.fail("worker_finalize_session_audio", new Error("Session is already finalizing"));
      env.app.fail("recording_transcribe", new Error("Could not read saved recording"));

      await env.dictation.toggleRecording();
      await settle();

      expect(env.app.named("worker_stop_session").map((call) => call.args)).toEqual([
        { sessionId: "s-1" },
        { sessionId: "s-2" },
      ]);
      expect(env.failures).toEqual([{ message: "Could not read saved recording", recordingSaved: true }]);
      expect(env.dictation.state).toBe("idle");
    });

    it("is transcribed with Standard when its session reports an error", async () => {
      const env = await liveDictationStopped();
      await env.dictation.toggleRecording();
      await settle();
      expect(env.app.named("worker_finalize_session_audio")).toHaveLength(1);

      await env.dictation.receiveSessionEvent(
        sessionEvent({ type: "error", text: "Realtime transcription session is closed" }, true),
      );
      await settle();

      await expectStandardTranscriptPasted(env);
    });

    it("is not transcribed again when it held no audio", async () => {
      const { app, dictation, failures } = await liveDictationStopped();
      await dictation.toggleRecording();
      await settle();

      await dictation.receiveSessionEvent(sessionEvent({ type: "error", text: "No audio found" }, true));
      await settle();

      expect(app.commands()).not.toContain("recording_transcribe");
      expect(failures).toEqual([{ message: "No audio found", recordingSaved: true }]);
      expect(dictation.state).toBe("idle");
    });

    it("falls back only once: a failed Standard transcription is reported", async () => {
      const { app, dictation, failures } = await liveDictationStopped();
      await dictation.toggleRecording();
      await settle();
      await dictation.receiveSessionEvent(
        sessionEvent({ type: "error", text: "Realtime transcription timed out" }, true),
      );
      await settle();

      await dictation.receiveSessionEvent({
        session_id: "s-2",
        event: { type: "error", text: "OpenAI is unreachable" },
        done: true,
      });

      expect(app.named("recording_transcribe")).toHaveLength(1);
      expect(failures).toEqual([{ message: "OpenAI is unreachable", recordingSaved: true }]);
      expect(app.named("recording_finish").map((call) => call.args)).toEqual([
        { recordingId: "rec-1", status: "failed", transcript: "", error: "OpenAI is unreachable" },
      ]);
      expect(dictation.state).toBe("idle");
    });

    it("stops the Standard session when cancelled while it starts", async () => {
      const { app, dictation, failures } = await liveDictationStopped();
      await dictation.toggleRecording();
      await settle();
      app.hold("worker_start_session");
      await dictation.receiveSessionEvent(
        sessionEvent({ type: "error", text: "Realtime transcription timed out" }, true),
      );
      await settle();

      await dictation.cancelTranscription();
      app.release("worker_start_session");
      await settle();

      expect(app.commands()).not.toContain("recording_transcribe");
      expect(app.named("worker_stop_session").at(-1)?.args).toEqual({ sessionId: "s-2" });
      expect(app.named("recording_finish").map((call) => call.args)).toEqual([
        { recordingId: "rec-1", status: "cancelled", transcript: "", error: "Cancelled by user" },
      ]);
      expect(failures).toEqual([]);
      expect(dictation.state).toBe("idle");
    });

    it("leaves a Standard transcription cancelled while it runs alone when it fails late", async () => {
      const { app, dictation, failures } = await liveDictationStopped();
      await dictation.toggleRecording();
      await settle();
      app.hold("recording_transcribe");
      await dictation.receiveSessionEvent(
        sessionEvent({ type: "error", text: "Realtime transcription timed out" }, true),
      );
      await settle();

      await dictation.cancelTranscription();
      app.reject("recording_transcribe", new Error("Session was stopped"));
      await settle();

      expect(app.named("worker_stop_session").map((call) => call.args)).toEqual([
        { sessionId: "s-1" },
        { sessionId: "s-2" },
      ]);
      expect(app.named("recording_finish").map((call) => call.args)).toEqual([
        { recordingId: "rec-1", status: "cancelled", transcript: "", error: "Cancelled by user" },
      ]);
      expect(failures).toEqual([]);
      expect(dictation.state).toBe("idle");
    });

    it("leaves the next recording alone when cancelled while its failure is saved", async () => {
      const { app, mic, dictation, failures } = await liveDictationStopped();
      app.fail("worker_finalize_session_audio", new Error("Session is already finalizing"));
      app.fail("recording_transcribe", new Error("Could not read saved recording"));
      app.hold("recording_finish");
      // The stop waits for the held failure save.
      void dictation.toggleRecording();
      await settle();
      expect(app.named("recording_finish")).toHaveLength(1);

      await dictation.cancelTranscription();
      app.answers.recording_begin = { id: "rec-2" };
      app.answers.worker_start_session = { session_id: "s-3" };
      await startRecording(dictation);
      app.release("recording_finish");
      await settle();

      expect(failures).toEqual([]);
      expect(dictation.state).toBe("recording");
      expect(mic.capture.isActive()).toBe(true);
    });
  });

  it("keeps recording for Standard when the Live connection cannot open", async () => {
    const { app, mic, dictation, failures } = setup(LIVE_TRANSCRIPTION_MODEL);
    app.hold("worker_start_session");
    void dictation.toggleRecording();
    await settle();
    mic.hear(BATCH);
    app.reject("worker_start_session", new Error("Realtime connection failed"));
    await settle();
    app.release("worker_start_session");
    await settle();

    expect(dictation.state).toBe("recording");
    expect(app.named("worker_start_session").map((call) => call.args)).toEqual([
      { profileId: "default" },
      { profileId: "default", standardOnly: true },
    ]);
    mic.hear(BATCH);
    await dictation.toggleRecording();
    await settle();

    expect(app.commands()).not.toContain("worker_append_audio_chunk");
    expect(app.commands()).not.toContain("worker_finalize_session_audio");
    expect(app.named("recording_append_chunk")).toHaveLength(2);
    expect(app.named("recording_transcribe").map((call) => call.args)).toEqual([
      { recordingId: "rec-1", sessionId: "s-1" },
    ]);
    expect(failures).toEqual([]);
  });

  it("transcribes imported and saved audio with Standard while Live is selected", async () => {
    const { app, dictation, failures } = setup(LIVE_TRANSCRIPTION_MODEL);

    await dictation.retranscribeRecording("rec-9");

    expect(failures).toEqual([]);
    expect(app.named("worker_start_session").map((call) => call.args)).toEqual([
      { profileId: "recording-retry", standardOnly: true },
    ]);
    expect(app.named("recording_transcribe").map((call) => call.args)).toEqual([
      { recordingId: "rec-9", sessionId: "s-1" },
    ]);
    await dictation.receiveSessionEvent(sessionEvent({ type: "final", text: "Again" }, true));
    expect(dictation.state).toBe("idle");

    await dictation.transcribeImportedFile();
    expect(failures).toEqual([]);
    expect(app.named("recording_transcribe").map((call) => call.args)).toContainEqual({
      recordingId: "imported-1",
      sessionId: "s-1",
    });
  });

  it("leaves a recording started during an import alone when the import's copy fails", async () => {
    const { app, mic, dictation, failures, toasts } = setup();
    app.hold("recording_import");
    const importing = dictation.transcribeImportedFile();
    await settle();

    await startRecording(dictation);
    mic.hear(100);
    app.reject("recording_import", new Error("Could not copy the file"));
    await importing;
    await settle();

    expect(failures).toEqual([]);
    expect(toasts).toEqual(["Could not copy the file"]);
    expect(dictation.state).toBe("recording");
    expect(mic.capture.isActive()).toBe(true);
    expect(app.commands()).not.toContain("recording_finish");

    await dictation.toggleRecording();
    await settle();
    expect(app.named("recording_transcribe").map((call) => call.args)).toEqual([
      { recordingId: "rec-1", sessionId: "s-1" },
    ]);
  });

  it("still shows a failed import's copy as a failure when nothing else runs", async () => {
    const { app, dictation, failures, toasts } = setup();
    app.fail("recording_import", new Error("Could not copy the file"));

    await dictation.transcribeImportedFile();

    expect(failures).toEqual([{ message: "Could not copy the file", recordingSaved: false }]);
    expect(toasts).toEqual([]);
    expect(dictation.state).toBe("idle");
  });

  it("leaves the next recording alone when a cancelled retry's session fails late", async () => {
    const { app, mic, dictation, failures } = setup();
    app.hold("worker_start_session");
    const retry = dictation.retranscribeRecording("rec-9");
    await settle();
    expect(dictation.state).toBe("transcribing");
    await dictation.cancelTranscription();

    app.answers.worker_start_session = { session_id: "s-2" };
    const recording = dictation.toggleRecording();
    await settle();
    expect(dictation.state).toBe("recording");
    // The cancelled retry's session request fails only now.
    app.reject("worker_start_session", new Error("Could not connect"));
    await retry;
    await settle();

    expect(failures).toEqual([]);
    expect(dictation.state).toBe("recording");
    expect(mic.capture.isActive()).toBe(true);
    expect(dictation.lastFailedRecordingId).toBeNull();
    expect(app.named("recording_finish").map((call) => call.args)).toEqual([
      { recordingId: "rec-9", status: "transcribing", transcript: "", error: "" },
      { recordingId: "rec-9", status: "cancelled", transcript: "", error: "Cancelled by user" },
    ]);
    app.release("worker_start_session");
    await recording;
    await dictation.toggleRecording();
    await settle();
    expect(app.named("recording_transcribe").map((call) => call.args)).toEqual([
      { recordingId: "rec-1", sessionId: "s-2" },
    ]);
  });

  it("leaves a retry cancelled during its transcription alone when it fails late", async () => {
    const { app, dictation, failures } = setup();
    app.hold("recording_transcribe");
    const retry = dictation.retranscribeRecording("rec-9");
    await settle();

    await dictation.cancelTranscription();
    app.reject("recording_transcribe", new Error("Session was stopped"));
    await retry;
    await settle();

    expect(app.named("worker_stop_session").map((call) => call.args)).toEqual([{ sessionId: "s-1" }]);
    expect(app.named("recording_finish").map((call) => call.args)).toEqual([
      { recordingId: "rec-9", status: "transcribing", transcript: "", error: "" },
      { recordingId: "rec-9", status: "cancelled", transcript: "", error: "Cancelled by user" },
    ]);
    expect(failures).toEqual([]);
    expect(dictation.lastFailedRecordingId).toBeNull();
    expect(dictation.state).toBe("idle");
  });

  it("reports a retry once when its session fails and ends before the transcribe command fails", async () => {
    const { app, dictation, failures } = setup();
    app.hold("recording_transcribe");
    const retry = dictation.retranscribeRecording("rec-9");
    await settle();

    await dictation.receiveSessionEvent(sessionEvent({ type: "error", text: "OpenAI is unreachable" }, true));
    app.reject("recording_transcribe", new Error("Session already ended"));
    await retry;
    await settle();

    expect(failures).toEqual([{ message: "OpenAI is unreachable", recordingSaved: true }]);
    expect(app.commands()).not.toContain("worker_stop_session");
    expect(dictation.state).toBe("idle");
  });

  it("reports a retry whose session fails while it is still current", async () => {
    const { app, dictation, failures } = setup();
    app.fail("worker_start_session", new Error("Could not connect"));

    await dictation.retranscribeRecording("rec-9");

    expect(failures).toEqual([{ message: "Could not connect", recordingSaved: true }]);
    expect(app.named("recording_finish").map((call) => call.args)).toEqual([
      { recordingId: "rec-9", status: "transcribing", transcript: "", error: "" },
      { recordingId: "rec-9", status: "failed", transcript: "", error: "Could not connect" },
    ]);
    expect(dictation.lastFailedRecordingId).toBe("rec-9");
    expect(dictation.state).toBe("idle");
  });

  it("stops a retry's session when transcribing the saved recording fails", async () => {
    const { app, dictation, failures } = setup();
    app.fail("recording_transcribe", new Error("Could not read saved recording"));

    await dictation.retranscribeRecording("rec-9");

    expect(app.named("worker_stop_session").map((call) => call.args)).toEqual([{ sessionId: "s-1" }]);
    expect(failures).toEqual([{ message: "Could not read saved recording", recordingSaved: true }]);
    expect(dictation.state).toBe("idle");
  });

  it("starts one transcription when a saved recording's retry is clicked twice", async () => {
    const { app, dictation, toasts } = setup();
    app.hold("recording_finish");

    const first = dictation.retranscribeRecording("rec-9");
    await settle();
    await dictation.retranscribeRecording("rec-8");
    await dictation.toggleRecording();
    app.release("recording_finish");
    await first;
    await settle();

    expect(toasts).toContain("Finish the current recording first");
    expect(app.named("recording_finish").map((call) => call.args)).toEqual([
      { recordingId: "rec-9", status: "transcribing", transcript: "", error: "" },
    ]);
    expect(app.named("worker_start_session")).toHaveLength(1);
    expect(app.commands()).not.toContain("recording_begin");
    expect(app.named("recording_transcribe").map((call) => call.args)).toEqual([
      { recordingId: "rec-9", sessionId: "s-1" },
    ]);
  });
  it("refuses an import while a dictation runs", async () => {
    const { app, dictation, toasts } = setup();
    await startRecording(dictation);

    await dictation.transcribeImportedFile();

    expect(toasts).toEqual(["Finish the current recording first"]);
    expect(app.commands()).not.toContain("recording_import");
    expect(dictation.state).toBe("recording");
  });

  it("refuses a picked file when a dictation started while the picker was open", async () => {
    const { app, dictation, toasts, failures } = setup();
    app.hold("recording_import");
    const importing = dictation.transcribeImportedFile();
    await settle();

    await startRecording(dictation);
    app.release("recording_import");
    await importing;

    expect(toasts).toEqual(["Finish the current recording first"]);
    expect(failures).toEqual([]);
    expect(app.commands()).not.toContain("recording_transcribe");
    expect(dictation.state).toBe("recording");
  });
});
