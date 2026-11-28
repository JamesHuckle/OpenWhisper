import audioRules from "../../../packages/protocol/audio-rules.json";
import { errorMessage } from "./error-message";
import { isLiveTranscription, type TranscriptionModel } from "./models";
import {
  joinAudioParts,
  sendAudioChunk,
  SessionAudioQueue,
  startCaptureFirst,
  type Invoke,
} from "./recording-pipeline";
import { deliverFinalTranscript, type TargetWrite } from "./target-write";

export type TranscriptEvent = { type: "partial" | "final" | "warning" | "error"; text: string };
// One update the app pushes for a session; `done` marks its last one.
export type WorkerSessionEvent = { session_id: string; event: TranscriptEvent; done?: boolean };
// "starting" shows the listening pill the moment the shortcut is pressed,
// while the microphone is still opening.
export type WidgetState = "idle" | "starting" | "recording" | "transcribing" | "error";

/** The settings a dictation reads when it starts. */
export type DictationSettings = {
  hasOpenaiApiKey: boolean;
  transcriptionModel: TranscriptionModel;
};

/** The microphone, captured as 16-bit PCM (pcm-capture.ts). */
export type AudioCapture = {
  /** Opens the microphone and sends its audio to onAudio as it is captured. */
  start: (onAudio: (chunk: ArrayBuffer) => void) => Promise<void>;
  /** True while capture runs. */
  isActive: () => boolean;
  /** Stops capture after sending its last samples to onAudio. */
  stop: () => Promise<void>;
  /** Stops capture at once, dropping unsent samples, and closes the microphone. */
  discard: () => void;
};

/** What the dictation flow needs from the app and the page. */
export type DictationDeps = {
  invoke: Invoke;
  capture: AudioCapture;
  /** The settings the menu shows, asked of the app only when unknown. */
  settings: () => Promise<DictationSettings>;
  /** True when the settings are loaded and have no OpenAI key. */
  apiKeyKnownMissing: () => boolean;
  /** Shows the new state on the pill. */
  setState: (next: WidgetState) => void;
  showToast: (message: string, durationMs?: number) => void;
  /** Shows a failed dictation: the pill notice and the menu's failure panel. */
  showFailure: (message: string, recordingSaved: boolean) => void;
  clearNotice: () => void;
  /** Capture runs: start the waveform and the silence check. */
  startMeter: () => void;
  /** Stops them. */
  stopMeter: () => void;
  /** Shows the transcript so far on the pill; final is true for a finished piece. */
  showTranscript: (text: string, final: boolean) => void;
  clearTranscript: () => void;
  /** A saved recording was added or changed. */
  recordingsChanged: () => void;
  closeMenu: () => void;
  log: (message: string) => void;
};

export type DictationController = ReturnType<typeof createDictationController>;

const AUDIO_BATCH_BYTES = 16 * 1024;

/**
 * The dictation flow: starting and stopping a recording, its saved backup,
 * its transcription session, cancel at any point, and delivering the result.
 * It holds every piece of its state; the page only shows it.
 */
export function createDictationController(deps: DictationDeps) {
  const { invoke, capture, log } = deps;

  let state: WidgetState = "idle";
  let currentSessionId: string | null = null;
  // Advanced as each dictation, import or retry starts and by every cancel.
  // Each one keeps its own number, so one that a cancel or a newer one
  // replaced stops its own late session instead of transcribing it, and its
  // late failure is only logged; it never stops, fails or resets the
  // recording that is current now.
  let operationId = 0;
  let sessionEventTail: Promise<void> = Promise.resolve();
  let liveTranscript = "";
  let streamTextToTarget = false;
  // Live transcription hears audio while it is recorded, so its session gets
  // every chunk, in order, through the dictation's own queue. Without one
  // (Standard, or once Live is given up) audio goes only to the saved backup,
  // and the transcriber reads that file once the recording stops. Each Live
  // dictation gets a new queue, so a replaced one's late uploads and failures
  // never reach the next.
  let liveAudioQueue: SessionAudioQueue | null = null;
  // The dictation's session is a Live one. If it fails, the saved recording
  // is transcribed with Standard instead, once (fellBackToStandard).
  let sessionWasLive = false;
  let fellBackToStandard = false;
  let targetWriteQueue: Promise<void> = Promise.resolve();
  let currentRecordingId: string | null = null;
  let lastFailedRecordingId: string | null = null;
  let backupTail: Promise<void> = Promise.resolve();
  // Settles once the latest start's recording_begin has answered and, if that
  // start was cancelled meanwhile, its file is closed. The next start opens
  // its file only then: the app closes every file left open as it opens a new
  // one, so an earlier begin answering later would close the newer file.
  let backupOpened: Promise<void> = Promise.resolve();
  let backupError: unknown = null;
  let pendingAudioParts: Uint8Array[] = [];
  let pendingAudioBytes = 0;
  // Set while a stop waits for the microphone's last samples; whatever
  // finishes the recording waits for them too, so they are saved and sent.
  // It never rejects: it settles once a failed stop has set captureStopError.
  let captureStopping: Promise<void> | null = null;
  // Set when the microphone failed to stop for the current recording. The
  // recording then fails, keeping the audio it received, once its session is
  // ready; it is never transcribed.
  let captureStopError: unknown = null;
  // Audio captured before the backup file opened, kept by the operation
  // number of the recording a cancel stopped, so the backup still saves it
  // once it opens. A stop still waiting for the microphone adds its last
  // samples to the same parts, so the backup waits for it first.
  const audioCancelledBeforeBackup = new Map<
    number,
    { parts: Uint8Array[]; stopping: Promise<void> | null }
  >();
  let shouldPasteFinalTranscript = true;
  let pressToTalkHeld = false;
  // The start each shortcut is running, if any. A cancel ends it at once, so
  // the next press starts a new recording, and the start it cancelled, once
  // it finishes, sees it is no longer current and leaves that recording alone.
  // stopRequested: the toggle was pressed again while it started.
  let holdStart: object | null = null;
  let toggleStart: { stopRequested: boolean } | null = null;
  // Set from when an import or retry is sure to run until the pill shows it
  // transcribing, so no other transcription or dictation starts meanwhile.
  let storedAudioStarting = false;
  // Set while a task runOnceIdle started runs (the update installing), so no
  // dictation, import or retry starts that it would interrupt.
  let startsRefused = false;
  // runOnceIdle's tasks waiting for the current dictation to finish.
  let tasksWaitingForIdle: (() => void)[] = [];

  function setState(next: WidgetState) {
    log(`setState ${state} -> ${next}`);
    state = next;
    deps.setState(next);
    if (isIdle()) for (const startTask of tasksWaitingForIdle.splice(0)) startTask();
  }

  function isIdle() {
    return state === "idle" && !storedAudioStarting;
  }

  // True, after telling the user why, while runOnceIdle's task runs.
  function refuseWhileStartsRefused(): boolean {
    if (startsRefused) deps.showToast("Installing the OpenWhisper update…");
    return startsRefused;
  }

  /**
   * Runs `task` once nothing is recording or transcribing: at once when idle,
   * otherwise as the current dictation, import or retry finishes. No new one
   * starts from then until `task` ends.
   */
  async function runOnceIdle<T>(task: () => Promise<T>): Promise<T> {
    await new Promise<void>((resolve) => {
      // Refused in the same step that sees idle, so nothing starts in between.
      const refuseStartsThenRun = () => {
        startsRefused = true;
        resolve();
      };
      if (isIdle()) refuseStartsThenRun();
      else tasksWaitingForIdle.push(refuseStartsThenRun);
    });
    try {
      return await task();
    } finally {
      startsRefused = false;
    }
  }

  // Read through a call, so a check after an await sees changes made meanwhile.
  function currentState(): WidgetState {
    return state;
  }

  // -------------------------------------------------------------------------
  // Transcription sessions
  // -------------------------------------------------------------------------
  /** Starts a session and returns its ID; standardOnly opens no Live connection. */
  // `live` is false when the app opened a Standard session, such as when
  // Standard was saved after this dictation chose Live.
  async function openSession(standardOnly = false) {
    return (await invoke(
      "worker_start_session",
      standardOnly ? { standardOnly } : {},
    )) as { session_id: string; live?: boolean };
  }

  async function startSession(standardOnly = false): Promise<string> {
    return (await openSession(standardOnly)).session_id;
  }

  /** Stops a session; a failure (the session may already be gone) is only logged. */
  async function stopSession(sessionId: string) {
    await invoke("worker_stop_session", { sessionId }).catch((error) => {
      log(`stop session failed=${errorMessage(error)}`);
    });
  }

  /**
   * Transcribes a saved recording with a new Standard session; the app reads
   * the file from disk, so the audio never loads into this window. `current`
   * says whether the caller's operation (and, given one, its session) is
   * still the controller's; once it is not, a cancel or the session's own
   * end already tidied up.
   */
  async function transcribeSavedRecording(
    recordingId: string,
    current: (sessionId?: string) => boolean,
  ) {
    const sessionId = await startSession(true);
    if (!current()) {
      // Cancelled while the session was starting; the cancel already tidied up.
      await stopSession(sessionId);
      return;
    }
    currentSessionId = sessionId;
    liveTranscript = "";
    try {
      await invoke("recording_transcribe", { recordingId, sessionId });
    } catch (error) {
      // Its failure is expected once the session is no longer this operation's.
      if (!current(sessionId)) return;
      // The app keeps the session until it is stopped when this fails before using it.
      await stopSession(sessionId);
      throw error;
    }
  }

  // -------------------------------------------------------------------------
  // Recording audio
  // -------------------------------------------------------------------------
  function queueAudioChunk(buffer: ArrayBuffer) {
    pendingAudioParts.push(new Uint8Array(buffer));
    pendingAudioBytes += buffer.byteLength;
    if (pendingAudioBytes >= AUDIO_BATCH_BYTES) flushAudioBatch();
  }

  function flushAudioBatch() {
    // Audio captured while the backup file is still opening waits here, and
    // is saved first once it opens.
    const recordingId = currentRecordingId;
    if (pendingAudioBytes === 0 || !recordingId) return;
    const combined = joinAudioParts(pendingAudioParts);
    pendingAudioParts = [];
    pendingAudioBytes = 0;

    backupTail = backupTail
      .then(() => sendAudioChunk(invoke, "recording_append_chunk", recordingId, combined))
      .catch((error) => {
        backupError ??= error;
      });
    liveAudioQueue?.append(combined.buffer);
  }

  // Writes a recording's final metadata; true once it is written. A failure
  // is only logged, and the recordings list is refreshed either way.
  async function finishRecording(recordingId: string, status: string, transcript: string, error: string) {
    let saved = true;
    try {
      await invoke("recording_finish", {
        recordingId,
        status,
        transcript,
        error,
      });
    } catch (finishError) {
      saved = false;
      log(`recording metadata update failed=${errorMessage(finishError)}`);
    }
    deps.recordingsChanged();
    return saved;
  }

  // True once the recording's metadata is written.
  async function finishCurrentRecording(status: string, transcript: string, error: string) {
    const recordingId = currentRecordingId;
    if (!recordingId) return false;
    currentRecordingId = null;
    return finishRecording(recordingId, status, transcript, error);
  }

  // False once `current` says a cancel or a newer recording took over, before
  // or while this waited; then it stops, leaving the microphone, the backup
  // and the pill to that one.
  async function handleTranscriptionFailure(error: unknown, current: () => boolean) {
    if (!current()) return false;
    if (capture.isActive()) {
      // A microphone that cannot stop cleanly is closed by cleanup.
      await capture.stop().catch((error) => {
        log(`capture stop failed=${errorMessage(error)}`);
      });
    }
    if (!current()) return false;
    flushAudioBatch();
    await backupTail;
    if (!current()) return false;
    const message = errorMessage(error);
    const recordingId = currentRecordingId;
    if (recordingId) {
      lastFailedRecordingId = recordingId;
      await finishCurrentRecording("failed", "", message);
      if (!current()) return false;
    }
    // Failures never open the menu or steal the flow: the pill briefly shows
    // a short notice in place, and the raw audio stays one click away.
    deps.showFailure(message, recordingId !== null);
    return true;
  }

  // Reports a failure and returns the pill to idle, unless `current` says a
  // cancel or a newer recording took over meanwhile.
  async function failAndReset(error: unknown, current: () => boolean) {
    if (!(await handleTranscriptionFailure(error, current)) || !current()) return;
    cleanup();
    setState("idle");
  }

  // -------------------------------------------------------------------------
  // Start, stop and cancel
  // -------------------------------------------------------------------------
  async function startRecording() {
    const startupStartedAt = performance.now();
    log(`startRecording enter state=${state} held=${pressToTalkHeld} starting=${holdStart !== null}`);
    if (state !== "idle" || storedAudioStarting || refuseWhileStartsRefused()) return;
    const op = ++operationId;
    // Settles this start's backupOpened; called on every way the start ends.
    let markBackupOpened = () => undefined as void;
    // Show the pill before anything is awaited, so it appears as the shortcut
    // is pressed. A missing key, when already known, shows only its toast.
    // The notice is cleared after, so a pill already showing one stays up.
    if (!deps.apiKeyKnownMissing()) setState("starting");
    deps.clearNotice();

    try {
      const settings = await deps.settings();
      // Cancelled or replaced meanwhile: the cancel already tidied up. A start
      // whose key was known missing leaves the pill idle, so runOnceIdle's
      // task may have begun meanwhile.
      if (op !== operationId || startsRefused) return;
      if (!settings.hasOpenaiApiKey) {
        deps.showToast("Set your OpenAI API key first (click arrow)");
        setState("idle");
        return;
      }
      const live = isLiveTranscription(settings.transcriptionModel);
      liveAudioQueue = live
        ? new SessionAudioQueue((sessionId, buffer) =>
            sendAudioChunk(invoke, "worker_append_audio_chunk", sessionId, buffer),
          )
        : null;
      sessionWasLive = false;
      fellBackToStandard = false;

      currentRecordingId = null;
      backupTail = Promise.resolve();
      backupError = null;
      captureStopError = null;
      pendingAudioParts = [];
      pendingAudioBytes = 0;
      shouldPasteFinalTranscript = true;
      // Set from this start's own session; kept only once the start is still
      // current, so a cancelled start that answers late never marks a newer
      // recording Live.
      let liveSession = false;
      const earlierBackupOpened = backupOpened;
      // Settles only after the earlier start's, so a start that fails before
      // opening its file never lets the next one open before an older one.
      backupOpened = new Promise((resolve) => {
        markBackupOpened = () => resolve(earlierBackupOpened);
      });

      const sessionId = await startCaptureFirst({
        lockPasteTarget: () => invoke("capture_paste_target"),
        // Capture is deliberately attached before the backup file, the session
        // or any network startup. Its opening chunks wait until those are ready.
        startCapture: () => capture.start(queueAudioChunk),
        onCaptureStarted: async () => {
          setState("recording");
          deps.startMeter();
          log(`capture ready startup_ms=${Math.round(performance.now() - startupStartedAt)}`);

          // A hold shortcut may be released, or a toggle shortcut may be pressed
          // again, while microphone permission/capture is still starting.
          if (op === operationId && ((holdStart && !pressToTalkHeld) || toggleStart?.stopRequested)) {
            await stopRecording();
          }
        },
        openBackup: async () => {
          await earlierBackupOpened;
          return invoke("recording_begin", {
            source: "microphone",
            name: `Recording ${new Date().toLocaleString()}`,
          }) as Promise<{ id: string }>;
        },
        onBackupOpened: async (backup) => {
          try {
            if (op !== operationId) {
              // Cancelled while the file was opening; the cancel already tidied
              // up, and a new recording may have started since.
              const cancelled = audioCancelledBeforeBackup.get(op);
              audioCancelledBeforeBackup.delete(op);
              if (cancelled?.stopping) await cancelled.stopping;
              const audio = cancelled?.parts;
              if (audio?.length) {
                await sendAudioChunk(
                  invoke,
                  "recording_append_chunk",
                  backup.id,
                  joinAudioParts(audio),
                ).catch((error) => {
                  log(`recording append failed=${errorMessage(error)}`);
                });
              }
              await finishRecording(backup.id, "cancelled", "", "Cancelled by user");
              return false;
            }
            currentRecordingId = backup.id;
            flushAudioBatch();
            return true;
          } finally {
            markBackupOpened();
          }
        },
        startSession: async () => {
          // The microphone failed to stop while this started: no session opens.
          if (captureStopError) throw captureStopError;
          if (live) {
            try {
              const session = await openSession();
              if (session.live !== false) liveSession = true;
              else if (op === operationId) {
                // Standard was saved since; it reads the saved recording.
                log("session opened as Standard; not streaming audio");
                liveAudioQueue = null;
              }
              return session.session_id;
            } catch (error) {
              if (op !== operationId) throw error;
              // The Live connection could not open. Recording goes on, and
              // the saved recording is transcribed with Standard on stop.
              log(`live session start failed=${errorMessage(error)}; using Standard`);
              liveAudioQueue = null;
              return startSession(true);
            }
          }
          // Standard was decided above; saying so keeps a Live model saved
          // since then from opening a Live connection this recording never uses.
          return startSession(true);
        },
      });
      if (!sessionId) return;
      if (op !== operationId) {
        // Cancelled while the session was starting; the cancel already tidied up.
        await stopSession(sessionId);
        return;
      }
      currentSessionId = sessionId;
      sessionWasLive = liveSession;
      liveAudioQueue?.attachSession(sessionId);
      liveTranscript = "";
      streamTextToTarget = sessionWasLive;
      targetWriteQueue = Promise.resolve();
      log(`session ready startup_ms=${Math.round(performance.now() - startupStartedAt)}`);

      if (currentState() === "transcribing") {
        // A stop still waiting for the microphone finalizes once it has its
        // last samples.
        if (captureStopping) return;
        await finalizeRecording();
        return;
      }
      log(`startRecording ready session=${currentSessionId ?? "none"} held=${pressToTalkHeld}`);
    } catch (err) {
      // A start that failed before its backup file opened never reached
      // onBackupOpened; the next start must not wait on it.
      markBackupOpened();
      audioCancelledBeforeBackup.delete(op);
      log(`startRecording error=${errorMessage(err)}`);
      // Unless a cancel already tidied this recording up: the microphone and
      // backup may now be a newer recording's.
      await failAndReset(err, () => op === operationId);
    }
  }

  async function stopRecording() {
    log(`stopRecording enter state=${state} session=${currentSessionId ?? "none"}`);
    if (state !== "recording") return;
    setState("transcribing");
    deps.stopMeter();
    const op = operationId;

    const stopping = (async () => {
      try {
        if (capture.isActive()) {
          await capture.stop();
          flushAudioBatch();
        }
      } catch (error) {
        log(`capture stop error=${errorMessage(error)}`);
        // Fails the recording below, once its session is ready, or in the
        // Standard fallback that started while this waited.
        if (op === operationId) {
          captureStopError = error;
          // The microphone is released now, not once the session is ready,
          // and no audio goes to a session this recording never uses. The
          // audio already received is still saved.
          capture.discard();
          liveAudioQueue = null;
        }
      }
    })();
    captureStopping = stopping;
    try {
      await stopping;
    } finally {
      if (captureStopping === stopping) captureStopping = null;
    }

    // A cancel meanwhile already tidied up; the session may now be a newer one.
    if (op !== operationId) return;
    // A Standard fallback that started while this waited finishes the recording.
    if (currentSessionId && !fellBackToStandard) await finalizeRecording();
  }

  async function finalizeRecording() {
    const sessionId = currentSessionId;
    if (!sessionId) return;
    const queue = liveAudioQueue;
    // False once a cancel, a newer recording or the Standard fallback took
    // this recording over while it waited; then that one finishes it.
    const owned = () => currentSessionId === sessionId && liveAudioQueue === queue;
    try {
      if (captureStopError) throw captureStopError;
      await queue?.drain();
      await backupTail;
      if (backupError) throw backupError;
      if (!owned()) return;
      if (queue) {
        await invoke("worker_finalize_session_audio", { sessionId });
      } else {
        // The transcriber reads the saved backup from disk, so the audio
        // crosses to it once, at the end, instead of a second time while
        // recording.
        const recordingId = currentRecordingId;
        if (!recordingId) throw new Error("The recording was not saved");
        await invoke("recording_transcribe", { recordingId, sessionId });
      }
      log(`stopRecording finalized session=${sessionId}`);
    } catch (err) {
      log(`stopRecording error=${errorMessage(err)}`);
      // Cancel answers at once, so a cancelled session's last request can
      // still fail here; the cancel or the Standard fallback handles it.
      if (!owned()) return;
      const message = errorMessage(err);
      if (canTranscribeWithStandard(message)) {
        await transcribeWithStandard(message);
        return;
      }
      // The app keeps the session, and a Live session its connection, until
      // it is stopped.
      await stopSession(sessionId);
      await failAndReset(err, owned);
    }
  }

  // A failed Live dictation is transcribed once more from its saved
  // recording, with Standard transcription; its final text replaces the
  // Live draft. Not when the recording held no speech, or was not saved whole.
  function canTranscribeWithStandard(error: string): boolean {
    return (
      state === "transcribing" &&
      sessionWasLive &&
      !fellBackToStandard &&
      currentRecordingId !== null &&
      backupError === null &&
      captureStopError === null &&
      error !== audioRules.no_audio_message
    );
  }

  async function transcribeWithStandard(error: string) {
    const recordingId = currentRecordingId;
    const failedSessionId = currentSessionId;
    if (!recordingId) return;
    fellBackToStandard = true;
    const op = operationId;
    log(`live transcription failed=${error}; transcribing the saved recording with Standard`);
    liveAudioQueue = null;
    streamTextToTarget = false;
    try {
      // The failed session may already be gone; stopping it closes its connection.
      if (failedSessionId) await stopSession(failedSessionId);
      // A failed Live upload gets here before the microphone's last samples
      // are written; the app closes the file as it reads it, so wait for them.
      await captureStopping;
      await backupTail;
      if (op !== operationId) return;
      // A microphone that failed to stop, before or during this wait, may not
      // have saved its last samples: the recording fails instead.
      if (captureStopError) throw captureStopError;
      if (backupError) throw backupError;
      await transcribeSavedRecording(recordingId, () => op === operationId);
    } catch (fallbackError) {
      log(`standard fallback error=${errorMessage(fallbackError)}`);
      // Unless a cancel already tidied up; then its failure is expected.
      await failAndReset(fallbackError, () => op === operationId);
    }
  }

  // A cancel still stopping its recording; a second press joins it, so it
  // never returns the pill to idle while the first can still tidy up a newer
  // recording.
  let cancelling: Promise<void> | null = null;

  function cancelTranscription(): Promise<void> {
    cancelling ??= cancelOnce().finally(() => {
      cancelling = null;
    });
    return cancelling;
  }

  async function cancelOnce() {
    const cancelledOp = operationId++;
    holdStart = null;
    toggleStart = null;
    if (!currentRecordingId && (pendingAudioParts.length > 0 || captureStopping)) {
      // The backup is still opening; it saves this audio once it opens.
      audioCancelledBeforeBackup.set(cancelledOp, {
        parts: pendingAudioParts,
        stopping: captureStopping,
      });
    }
    // Audio still waiting for a full batch is saved with the recording, and
    // so are the microphone's last samples when a stop is waiting for them.
    // None of it goes to the stopping session.
    liveAudioQueue = null;
    flushAudioBatch();
    const stopping = captureStopping;
    const sessionId = currentSessionId;
    // The session stops being this recording's at once, so a result that
    // arrives while it stops is not pasted or copied.
    currentSessionId = null;
    if (sessionId) await stopSession(sessionId);
    if (stopping) await stopping;
    // Capture has stopped; let its last pieces reach the backup before closing it.
    await backupTail;
    await finishCurrentRecording("cancelled", "", "Cancelled by user");
    cleanup();
    setState("idle");
    deps.showToast("Transcription cancelled. Raw audio is still saved.", 4000);
  }

  // -------------------------------------------------------------------------
  // Saved audio: imported files and retries
  // -------------------------------------------------------------------------
  // Imports and retries wait for the controller; true when it is busy, after
  // telling the user so.
  function refuseWhileBusy(): boolean {
    if (refuseWhileStartsRefused()) return true;
    if (isIdle()) return false;
    deps.showToast("Finish the current recording first");
    return true;
  }

  async function transcribeImportedFile() {
    if (refuseWhileBusy()) return;
    // Set once the import reserves the controller; until then it owns none of
    // the current recording's state.
    let op: number | null = null;
    try {
      const settings = await deps.settings();
      if (!settings.hasOpenaiApiKey) {
        throw new Error("Set your OpenAI API key before importing audio");
      }
      // The native picker hands the path to the app, which copies the file
      // from disk; the audio never passes through this window.
      const stored = (await invoke("recording_import")) as { id: string } | null;
      if (!stored) return;
      if (refuseWhileBusy()) return;
      storedAudioStarting = true;
      op = ++operationId;
      await transcribeStoredAudio(stored.id, op);
    } catch (error) {
      if (op === null) {
        // The picker or the copy failed before the import started; a
        // dictation may have started meanwhile, and stays untouched.
        const message = errorMessage(error);
        log(`import error=${message}`);
        if (currentState() === "idle" && !storedAudioStarting) deps.showFailure(message, false);
        else deps.showToast(message);
        return;
      }
      await failStoredAudio(op, error);
    }
  }

  async function retranscribeRecording(recordingId: string) {
    if (refuseWhileBusy()) return;
    storedAudioStarting = true;
    const op = ++operationId;
    try {
      const settings = await deps.settings();
      if (!settings.hasOpenaiApiKey) {
        throw new Error("Set your OpenAI API key before transcribing saved audio");
      }
      await transcribeStoredAudio(recordingId, op);
    } catch (error) {
      await failStoredAudio(op, error);
    }
  }

  // Reports a failed import or retry, unless a cancel replaced it; then the
  // failure is expected, and the controller may hold a newer recording.
  async function failStoredAudio(op: number, error: unknown) {
    if (op !== operationId) {
      log(`saved audio error after cancel=${errorMessage(error)}`);
      return;
    }
    storedAudioStarting = false;
    await failAndReset(error, () => op === operationId);
  }

  async function transcribeStoredAudio(recordingId: string, op: number) {
    deps.clearNotice();
    currentRecordingId = recordingId;
    lastFailedRecordingId = null;
    sessionWasLive = false;
    shouldPasteFinalTranscript = false;
    liveTranscript = "";
    targetWriteQueue = Promise.resolve();
    await invoke("recording_finish", {
      recordingId,
      status: "transcribing",
      transcript: "",
      error: "",
    });
    setState("transcribing");
    storedAudioStarting = false;
    deps.closeMenu();

    // Saved audio is always transcribed with Standard, so no Live
    // connection is opened even when Live is selected.
    await transcribeSavedRecording(
      recordingId,
      (sessionId) => op === operationId && (!sessionId || currentSessionId === sessionId),
    );
  }

  function cleanup() {
    log(`cleanup state=${state} session=${currentSessionId ?? "none"}`);
    sessionWasLive = false;
    deps.stopMeter();
    capture.discard();
    currentSessionId = null;
    liveAudioQueue = null;
    pendingAudioParts = [];
    pendingAudioBytes = 0;
    liveTranscript = "";
    streamTextToTarget = false;
  }

  // -------------------------------------------------------------------------
  // Session events: the app pushes progress, live text and the result as
  // they happen. They are handled one at a time, in the order they arrive.
  // -------------------------------------------------------------------------
  function queueSessionWork(work: () => Promise<void>): Promise<void> {
    sessionEventTail = sessionEventTail.then(work).catch((error) => {
      log(`session event failed=${errorMessage(error)}`);
    });
    return sessionEventTail;
  }

  // A final or error event is a session's last one: it ends the dictation.
  async function handleSessionEvent(message: WorkerSessionEvent) {
    const { session_id: sessionId, event } = message;
    if (!currentSessionId || sessionId !== currentSessionId) return;
    try {
      if (event.type === "final") {
        deps.showTranscript(event.text, true);
        await completeSession(sessionId, event.text.trim());
      } else if (event.type === "error") {
        await failSession(sessionId, event.text || "Transcription failed");
      } else if (event.type === "warning") {
        deps.showToast(event.text, 8000);
      } else {
        showLiveText(event.text);
      }
    } catch (error) {
      const text = errorMessage(error, "Could not finish the transcription");
      log(`session event failed=${text}`);
      if (message.done && (state === "recording" || state === "transcribing")) {
        await failAndReset(text, () => currentSessionId === sessionId);
      }
    }
  }

  function showLiveText(text: string) {
    if (text === "Refining transcript..." || text.startsWith("Transcribing part ")) return;
    liveTranscript += text;
    deps.showTranscript(liveTranscript, false);
    if (!streamTextToTarget) return;
    // A piece still queued when its session is cancelled is never typed
    // into the field a later recording locked.
    const sessionId = currentSessionId;
    targetWriteQueue = targetWriteQueue.then(async () => {
      if (!streamTextToTarget || currentSessionId !== sessionId) return;
      // The native side checks the field before each piece. Once it stops
      // (field changed, cursor moved, password field), the final text is
      // pasted without deleting what the user did.
      const write = (await invoke("type_to_target", { text })) as TargetWrite;
      if (currentSessionId !== sessionId) return;
      if (write.outcome !== "typed") {
        streamTextToTarget = false;
        log(`live target typing stopped reason=${write.reason ?? "unknown"}`);
      }
    }).catch((err) => {
      log(`live target write failed=${errorMessage(err)}`);
    });
  }

  async function completeSession(sessionId: string, transcript: string) {
    // A cancel (or a new recording) while this waits takes the session over;
    // then nothing more is written, recorded or cleaned up here.
    const owned = () => currentSessionId === sessionId;
    if (transcript) {
      // Saved before it is copied or pasted, so a delivery error never turns a
      // paid transcription into a failed recording.
      await deliverFinalTranscript({
        invoke,
        transcript,
        paste: shouldPasteFinalTranscript,
        owned,
        saveRecording: (saved) => finishCurrentRecording("complete", saved, ""),
        liveTypingDone: () => targetWriteQueue,
        notify: deps.showToast,
        log,
      });
    }
    if (owned()) endSession();
  }

  // A failed Live session hands its saved recording to Standard instead.
  async function failSession(sessionId: string, failure: string) {
    if (canTranscribeWithStandard(failure)) {
      // Not awaited, so the new session's events are handled as they arrive.
      void transcribeWithStandard(failure).catch((error) => {
        log(`standard fallback failed=${errorMessage(error)}`);
      });
      return;
    }
    if (await handleTranscriptionFailure(failure, () => currentSessionId === sessionId)) endSession();
  }

  function endSession() {
    cleanup();
    deps.clearTranscript();
    setState("idle");
  }

  // -------------------------------------------------------------------------
  // Shortcuts
  // -------------------------------------------------------------------------
  async function toggleRecording() {
    // A second press while the microphone is still opening stops the recording
    // as soon as it starts (and a third press keeps it going after all).
    if (toggleStart && (state === "idle" || state === "starting")) {
      toggleStart.stopRequested = !toggleStart.stopRequested;
      return;
    }
    if (state === "idle") {
      const start = { stopRequested: false };
      toggleStart = start;
      try {
        await startRecording();
        if (toggleStart === start && start.stopRequested && currentState() === "recording") {
          await stopRecording();
        }
      } finally {
        if (toggleStart === start) toggleStart = null;
      }
    } else if (state === "recording") {
      await stopRecording();
    }
  }

  async function pressToTalkStart() {
    log(`pressToTalkStart event state=${state} held=${pressToTalkHeld} starting=${holdStart !== null}`);
    pressToTalkHeld = true;
    if (state !== "idle" || holdStart) return;

    const start = {};
    holdStart = start;
    // Set when the user released the shortcut while this start was still in
    // flight and it is still current, so it stops once recording is active.
    let releasedDuringStart = false;
    try {
      await startRecording();
      releasedDuringStart = holdStart === start && !pressToTalkHeld && capture.isActive();
    } finally {
      if (holdStart === start) holdStart = null;
    }

    if (releasedDuringStart) {
      log("pressToTalkStart detected release during startup");
      await stopRecording();
    }
  }

  async function pressToTalkStop() {
    log(`pressToTalkStop event state=${state} held=${pressToTalkHeld} starting=${holdStart !== null}`);
    pressToTalkHeld = false;
    if (state === "recording") {
      await stopRecording();
    }
  }

  // Native end-to-end smoke run for development builds. It deliberately uses
  // the same Tauri commands, session events, and target-field streaming as a
  // microphone recording; only the PCM source is deterministic test audio.
  async function runRealtimeSmoke(chunks: string[], intervalMs = 100) {
    if (state !== "idle") throw new Error(`Cannot start smoke test while state=${state}`);
    // Only a Live session takes audio chunks; Standard reads the saved file.
    const settings = await deps.settings();
    if (!isLiveTranscription(settings.transcriptionModel)) {
      throw new Error("Choose Live transcription before the realtime smoke test");
    }
    await invoke("capture_paste_target");
    const sessionId = await startSession();
    currentSessionId = sessionId;
    liveTranscript = "";
    streamTextToTarget = true;
    targetWriteQueue = Promise.resolve();
    setState("recording");
    for (const chunkBase64 of chunks) {
      // The test audio arrives as base64 text; the app sends raw bytes.
      const chunk = Uint8Array.from(atob(chunkBase64), (char) => char.charCodeAt(0));
      await sendAudioChunk(invoke, "worker_append_audio_chunk", sessionId, chunk);
      await new Promise((resolve) => setTimeout(resolve, intervalMs));
    }
    setState("transcribing");
    await invoke("worker_finalize_session_audio", { sessionId });
  }

  return {
    get state(): WidgetState {
      return state;
    },
    /** The recording being made or transcribed now, if any. */
    get currentRecordingId(): string | null {
      return currentRecordingId;
    },
    /** The last recording whose transcription failed, kept to open its audio. */
    get lastFailedRecordingId(): string | null {
      return lastFailedRecordingId;
    },
    runOnceIdle,
    toggleRecording,
    stopRecording,
    cancelTranscription,
    pressToTalkStart,
    pressToTalkStop,
    transcribeImportedFile,
    retranscribeRecording,
    /** Queues a pushed session event; resolves once it has been handled. */
    receiveSessionEvent: (message: WorkerSessionEvent) =>
      queueSessionWork(() => handleSessionEvent(message)),
    runRealtimeSmoke,
  };
}
