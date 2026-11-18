import { isLiveTranscription, type TranscriptionModel } from "./models";
import { PCM_MIME_TYPE } from "./pcm-capture";
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
  /** Stops them; waiting is true when the pill now waits for the transcript. */
  stopMeter: (waiting: boolean) => void;
  /** Shows the transcript so far on the pill; final is true for a finished piece. */
  showTranscript: (text: string, final: boolean) => void;
  clearTranscript: () => void;
  /** Keeps a delivered transcript as the last one. */
  rememberTranscript: (transcript: string) => void;
  /** A saved recording was added or changed. */
  recordingsChanged: () => void;
  closeMenu: () => void;
  log: (message: string) => void;
};

export type DictationController = ReturnType<typeof createDictationController>;

const AUDIO_BATCH_BYTES = 16 * 1024;

export function errorMessage(error: unknown, fallback = "Transcription failed"): string {
  if (error instanceof Error && error.message.trim()) return error.message;
  const message = String(error ?? "").trim();
  return message || fallback;
}

/**
 * The dictation flow: starting and stopping a recording, its saved backup,
 * its transcription session, cancel at any point, and delivering the result.
 * It holds every piece of its state; the page only shows it.
 */
export function createDictationController(deps: DictationDeps) {
  const { invoke, capture, log } = deps;

  let state: WidgetState = "idle";
  let currentSessionId: string | null = null;
  // Advanced by every cancel, so a session that was still starting when the
  // user cancelled is stopped instead of transcribed.
  let cancelCount = 0;
  let sessionEventTail: Promise<void> = Promise.resolve();
  let finalTranscript = "";
  let liveTranscript = "";
  let streamTextToTarget = false;
  // Live transcription hears audio while it is recorded, so its session gets
  // every chunk. Otherwise audio goes only to the saved backup, and the
  // transcriber reads that file once the recording stops.
  let streamAudioToWorker = false;
  let targetWriteQueue: Promise<void> = Promise.resolve();
  let currentRecordingId: string | null = null;
  let lastFailedRecordingId: string | null = null;
  let backupTail: Promise<void> = Promise.resolve();
  let backupError: unknown = null;
  let pendingAudioParts: Uint8Array[] = [];
  let pendingAudioBytes = 0;
  // Audio captured before the backup file opened, kept by the number of the
  // cancel that stopped its recording, so the backup still saves it once it opens.
  const audioCancelledBeforeBackup = new Map<number, Uint8Array[]>();
  let sessionFailure = "";
  let shouldPasteFinalTranscript = true;
  let pressToTalkHeld = false;
  let pressToTalkStarting = false;
  let toggleRecordingStarting = false;
  let toggleStopRequestedDuringStart = false;
  const audioQueue = new SessionAudioQueue((sessionId, buffer) =>
    sendAudioChunk(invoke, "worker_append_audio_chunk", sessionId, buffer),
  );

  function setState(next: WidgetState) {
    log(`setState ${state} -> ${next}`);
    state = next;
    deps.setState(next);
  }

  // Read through a call, so a check after an await sees changes made meanwhile.
  function currentState(): WidgetState {
    return state;
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
    if (streamAudioToWorker) audioQueue.append(combined.buffer);
  }

  async function finishCurrentRecording(status: string, transcript: string, error: string) {
    const recordingId = currentRecordingId;
    if (!recordingId) return;
    currentRecordingId = null;
    try {
      await invoke("recording_finish", {
        recordingId,
        status,
        transcript,
        error,
      });
    } catch (finishError) {
      log(`recording metadata update failed=${errorMessage(finishError)}`);
    }
    deps.recordingsChanged();
  }

  async function handleTranscriptionFailure(error: unknown) {
    if (capture.isActive()) await capture.stop();
    flushAudioBatch();
    await backupTail;
    const message = errorMessage(error);
    const recordingId = currentRecordingId;
    if (recordingId) {
      lastFailedRecordingId = recordingId;
      await finishCurrentRecording("failed", "", message);
    }
    // Failures never open the menu or steal the flow: the pill briefly shows
    // a short notice in place, and the raw audio stays one click away.
    deps.showFailure(message, recordingId !== null);
  }

  // -------------------------------------------------------------------------
  // Start, stop and cancel
  // -------------------------------------------------------------------------
  async function startRecording() {
    const startupStartedAt = performance.now();
    log(`startRecording enter state=${state} held=${pressToTalkHeld} starting=${pressToTalkStarting}`);
    if (state !== "idle") return;
    const cancelsBefore = cancelCount;
    // Show the pill before anything is awaited, so it appears as the shortcut
    // is pressed. A missing key, when already known, shows only its toast.
    // The notice is cleared after, so a pill already showing one stays up.
    if (!deps.apiKeyKnownMissing()) setState("starting");
    deps.clearNotice();

    try {
      const settings = await deps.settings();
      // Cancelled meanwhile: the cancel already tidied up.
      if (cancelCount !== cancelsBefore) return;
      if (!settings.hasOpenaiApiKey) {
        deps.showToast("Set your OpenAI API key first (click arrow)");
        setState("idle");
        return;
      }
      streamAudioToWorker = isLiveTranscription(settings.transcriptionModel);

      currentRecordingId = null;
      backupTail = Promise.resolve();
      backupError = null;
      pendingAudioParts = [];
      pendingAudioBytes = 0;
      sessionFailure = "";
      shouldPasteFinalTranscript = true;

      const result = await startCaptureFirst({
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
          if ((pressToTalkStarting && !pressToTalkHeld) || toggleStopRequestedDuringStart) {
            await stopRecording();
          }
        },
        openBackup: () => invoke("recording_begin", {
          source: "microphone",
          name: `Recording ${new Date().toLocaleString()}`,
        }) as Promise<{ id: string }>,
        onBackupOpened: async (backup) => {
          if (cancelCount !== cancelsBefore) {
            // Cancelled while the file was opening; the cancel already tidied
            // up, and a new recording may have started since.
            const audio = audioCancelledBeforeBackup.get(cancelsBefore + 1);
            audioCancelledBeforeBackup.delete(cancelsBefore + 1);
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
            await invoke("recording_finish", {
              recordingId: backup.id,
              status: "cancelled",
              transcript: "",
              error: "Cancelled by user",
            }).catch((error) => {
              log(`recording metadata update failed=${errorMessage(error)}`);
            });
            deps.recordingsChanged();
            return false;
          }
          currentRecordingId = backup.id;
          flushAudioBatch();
          return true;
        },
        startSession: () => invoke("worker_start_session", {
          profileId: "default",
        }) as Promise<{ session_id: string }>,
      });
      if (!result) return;
      if (cancelCount !== cancelsBefore) {
        // Cancelled while the session was starting; the cancel already tidied up.
        await invoke("worker_stop_session", { sessionId: result.session_id }).catch((error) => {
          log(`stop session failed=${errorMessage(error)}`);
        });
        return;
      }
      currentSessionId = result.session_id;
      audioQueue.attachSession(currentSessionId);
      finalTranscript = "";
      liveTranscript = "";
      streamTextToTarget = isLiveTranscription(settings.transcriptionModel);
      targetWriteQueue = Promise.resolve();
      log(`session ready startup_ms=${Math.round(performance.now() - startupStartedAt)}`);

      if (currentState() === "transcribing") {
        await finalizeRecording();
        return;
      }
      log(`startRecording ready session=${currentSessionId ?? "none"} held=${pressToTalkHeld}`);
    } catch (err) {
      audioCancelledBeforeBackup.delete(cancelsBefore + 1);
      log(`startRecording error=${errorMessage(err)}`);
      // A cancel already tidied this recording up, and the microphone and
      // backup may now be a newer recording's.
      if (cancelCount !== cancelsBefore) return;
      if (capture.isActive()) await capture.stop();
      flushAudioBatch();
      await backupTail;
      await handleTranscriptionFailure(err);
      cleanup();
      setState("idle");
    }
  }

  async function stopRecording() {
    log(`stopRecording enter state=${state} session=${currentSessionId ?? "none"}`);
    if (state !== "recording") return;
    setState("transcribing");
    deps.stopMeter(true);

    if (capture.isActive()) {
      await capture.stop();
      flushAudioBatch();
    }

    if (currentSessionId) await finalizeRecording();
  }

  async function finalizeRecording() {
    const sessionId = currentSessionId;
    if (!sessionId) return;
    try {
      await audioQueue.drain();
      await backupTail;
      if (backupError) throw backupError;
      if (currentSessionId !== sessionId) return;
      if (streamAudioToWorker) {
        await invoke("worker_finalize_session_audio", {
          sessionId,
          mimeType: PCM_MIME_TYPE,
        });
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
      // still fail here; the cancel already tidied up.
      if (currentSessionId !== sessionId) return;
      await handleTranscriptionFailure(err);
      cleanup();
      setState("idle");
    }
  }

  async function cancelTranscription() {
    cancelCount += 1;
    if (!currentRecordingId && pendingAudioParts.length > 0) {
      // The backup is still opening; it saves this audio once it opens.
      audioCancelledBeforeBackup.set(cancelCount, pendingAudioParts);
    }
    const sessionId = currentSessionId;
    // The session stops being this recording's at once, so a result that
    // arrives while it stops is not pasted or copied.
    currentSessionId = null;
    if (sessionId) {
      await invoke("worker_stop_session", { sessionId }).catch((error) => {
        log(`stop session failed=${errorMessage(error)}`);
      });
    }
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
  async function transcribeImportedFile() {
    if (state !== "idle") {
      deps.showToast("Finish the current recording first");
      return;
    }
    try {
      const settings = await deps.settings();
      if (!settings.hasOpenaiApiKey) {
        throw new Error("Set your OpenAI API key before importing audio");
      }
      if (isLiveTranscription(settings.transcriptionModel)) {
        throw new Error("Choose Standard transcription for imported audio files");
      }
      // The native picker hands the path to the app, which copies the file
      // from disk; the audio never passes through this window.
      const stored = (await invoke("recording_import")) as { id: string } | null;
      if (!stored) return;
      if (currentState() !== "idle") {
        deps.showToast("Finish the current recording first");
        return;
      }
      await transcribeStoredAudio(stored.id, "file-import");
    } catch (error) {
      await handleTranscriptionFailure(error);
      cleanup();
      setState("idle");
    }
  }

  async function retranscribeRecording(recordingId: string) {
    if (state !== "idle") {
      deps.showToast("Finish the current recording first");
      return;
    }
    try {
      const settings = await deps.settings();
      if (!settings.hasOpenaiApiKey) {
        throw new Error("Set your OpenAI API key before transcribing saved audio");
      }
      if (isLiveTranscription(settings.transcriptionModel)) {
        throw new Error("Choose Standard transcription for saved audio files");
      }
      await transcribeStoredAudio(recordingId, "recording-retry");
    } catch (error) {
      await handleTranscriptionFailure(error);
      cleanup();
      setState("idle");
    }
  }

  async function transcribeStoredAudio(recordingId: string, profileId: string) {
    deps.clearNotice();
    currentRecordingId = recordingId;
    lastFailedRecordingId = null;
    sessionFailure = "";
    shouldPasteFinalTranscript = false;
    finalTranscript = "";
    liveTranscript = "";
    targetWriteQueue = Promise.resolve();
    await invoke("recording_finish", {
      recordingId,
      status: "transcribing",
      transcript: "",
      error: "",
    });
    setState("transcribing");
    deps.closeMenu();

    const cancelsBefore = cancelCount;
    const session = (await invoke("worker_start_session", {
      profileId,
    })) as { session_id: string };
    const sessionId = session.session_id;
    if (cancelCount !== cancelsBefore) {
      // Cancelled while the session was starting; the cancel already tidied up.
      await invoke("worker_stop_session", { sessionId }).catch((error) => {
        log(`stop session failed=${errorMessage(error)}`);
      });
      return;
    }
    currentSessionId = sessionId;
    const cancelled = () => currentSessionId !== sessionId;
    try {
      // The app finds the saved file and reads it from disk, so the audio is
      // never loaded into this window.
      await invoke("recording_transcribe", { recordingId, sessionId });
    } catch (error) {
      // A cancel during the transcription already tidied up; its failure is expected.
      if (cancelled()) return;
      throw error;
    }
  }

  function cleanup() {
    log(`cleanup state=${state} session=${currentSessionId ?? "none"}`);
    deps.stopMeter(false);
    capture.discard();
    currentSessionId = null;
    audioQueue.reset();
    pendingAudioParts = [];
    pendingAudioBytes = 0;
    liveTranscript = "";
    streamTextToTarget = false;
    streamAudioToWorker = false;
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

  async function handleSessionEvent(message: WorkerSessionEvent) {
    if (!currentSessionId || message.session_id !== currentSessionId) return;
    try {
      await applyTranscriptEvent(message.event);
      if (message.done) await completeSession(message.session_id);
    } catch (error) {
      const text = errorMessage(error, "Could not finish the transcription");
      log(`session event failed=${text}`);
      if (message.done && (state === "recording" || state === "transcribing")) {
        await handleTranscriptionFailure(text);
        cleanup();
        setState("idle");
      }
    }
  }

  async function applyTranscriptEvent(ev: TranscriptEvent) {
    if (ev.type === "final") {
      finalTranscript = `${finalTranscript} ${ev.text}`.trim();
      liveTranscript = ev.text;
      deps.showTranscript(liveTranscript, true);
    } else if (
      ev.type === "partial" &&
      ev.text !== "Refining transcript..." &&
      !ev.text.startsWith("Transcribing part ")
    ) {
      liveTranscript += ev.text;
      deps.showTranscript(liveTranscript, false);
      if (streamTextToTarget) {
        const delta = ev.text;
        // A piece still queued when its session is cancelled is never typed
        // into the field a later recording locked.
        const sessionId = currentSessionId;
        targetWriteQueue = targetWriteQueue.then(async () => {
          if (!streamTextToTarget || currentSessionId !== sessionId) return;
          // The native side checks the field before each piece. Once it stops
          // (field changed, cursor moved, password field), the final text is
          // pasted without deleting what the user did.
          const write = (await invoke("type_to_target", { text: delta })) as TargetWrite;
          if (currentSessionId !== sessionId) return;
          if (write.outcome !== "typed") {
            streamTextToTarget = false;
            log(`live target typing stopped reason=${write.reason ?? "unknown"}`);
          }
        }).catch((err) => {
          log(`live target write failed=${errorMessage(err)}`);
        });
      }
    } else if (ev.type === "error") {
      sessionFailure = ev.text || "Transcription failed";
      await handleTranscriptionFailure(sessionFailure);
    } else if (ev.type === "warning") {
      deps.showToast(ev.text, 8000);
    }
  }

  async function completeSession(sessionId: string) {
    // A cancel (or a new recording) while this waits takes the session over;
    // then nothing more is written, recorded or cleaned up here.
    const owned = () => currentSessionId === sessionId;
    if (finalTranscript) {
      // Saved before it is copied or pasted, so a delivery error never turns a
      // paid transcription into a failed recording.
      await deliverFinalTranscript({
        invoke,
        transcript: finalTranscript,
        paste: shouldPasteFinalTranscript,
        owned,
        saveRecording: (transcript) => finishCurrentRecording("complete", transcript, ""),
        rememberLocally: deps.rememberTranscript,
        liveTypingDone: () => targetWriteQueue,
        notify: deps.showToast,
        log,
      });
    } else if (sessionFailure) {
      await finishCurrentRecording("failed", "", sessionFailure);
    }
    if (!owned()) return;
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
    if (toggleRecordingStarting && (state === "idle" || state === "starting")) {
      toggleStopRequestedDuringStart = !toggleStopRequestedDuringStart;
      return;
    }
    if (state === "idle") {
      toggleRecordingStarting = true;
      toggleStopRequestedDuringStart = false;
      try {
        await startRecording();
        if (toggleStopRequestedDuringStart && currentState() === "recording") {
          await stopRecording();
        }
      } finally {
        toggleRecordingStarting = false;
        toggleStopRequestedDuringStart = false;
      }
    } else if (state === "recording") {
      await stopRecording();
    }
  }

  async function pressToTalkStart() {
    log(`pressToTalkStart event state=${state} held=${pressToTalkHeld} starting=${pressToTalkStarting}`);
    pressToTalkHeld = true;
    if (state !== "idle" || pressToTalkStarting) return;

    pressToTalkStarting = true;
    try {
      await startRecording();
    } finally {
      pressToTalkStarting = false;
    }

    // If the user released the shortcut while startup was still in flight,
    // stop immediately once recording is actually active.
    if (!pressToTalkHeld && capture.isActive()) {
      log("pressToTalkStart detected release during startup");
      await stopRecording();
    }
  }

  async function pressToTalkStop() {
    log(`pressToTalkStop event state=${state} held=${pressToTalkHeld} starting=${pressToTalkStarting}`);
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
    const result = (await invoke("worker_start_session", {
      profileId: "realtime-smoke",
    })) as { session_id: string };
    const sessionId = result.session_id;
    currentSessionId = sessionId;
    finalTranscript = "";
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
    await invoke("worker_finalize_session_audio", {
      sessionId,
      mimeType: PCM_MIME_TYPE,
    });
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
