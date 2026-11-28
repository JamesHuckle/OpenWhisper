import type { InvokeArgs, InvokeOptions } from "@tauri-apps/api/core";

export type AudioChunkUploader = (sessionId: string, buffer: ArrayBuffer) => Promise<unknown>;

/** The shape of Tauri's invoke, so tests can pass a fake. */
export type Invoke = (
  command: string,
  args?: InvokeArgs,
  options?: InvokeOptions,
) => Promise<unknown>;

// Which header names the recording or session an audio chunk belongs to.
const AUDIO_CHUNK_ID_HEADERS = {
  recording_append_chunk: "recording-id",
  worker_append_audio_chunk: "session-id",
} as const;

export type AudioChunkCommand = keyof typeof AUDIO_CHUNK_ID_HEADERS;

/**
 * Sends audio to the shell as raw bytes, with its recording or session id in
 * a header. The page never turns audio into base64 text: the shell writes the
 * bytes to the WAV file as they are, and sends Live audio on to OpenAI.
 */
export async function sendAudioChunk(
  invoke: Invoke,
  command: AudioChunkCommand,
  id: string,
  chunk: ArrayBuffer | Uint8Array,
): Promise<void> {
  await invoke(command, chunk, { headers: { [AUDIO_CHUNK_ID_HEADERS[command]]: id } });
}

/** Joins captured audio parts into one chunk, in order. */
export function joinAudioParts(parts: Uint8Array[]): Uint8Array<ArrayBuffer> {
  const combined = new Uint8Array(parts.reduce((bytes, part) => bytes + part.byteLength, 0));
  let offset = 0;
  for (const part of parts) {
    combined.set(part, offset);
    offset += part.byteLength;
  }
  return combined;
}

/**
 * Starts a recording with the microphone first, so words said right after the
 * shortcut are kept. The paste target is locked at once, alongside capture;
 * the backup file opens once capture runs, and the transcription session starts
 * last, after both. The caller holds the audio captured before the backup opens.
 * onBackupOpened returns false when the recording was cancelled meanwhile;
 * no session is started then and the result is null.
 */
export async function startCaptureFirst<Capture, Backup, Session>(steps: {
  lockPasteTarget: () => Promise<unknown>;
  startCapture: () => Promise<Capture>;
  onCaptureStarted: (capture: Capture) => void | Promise<void>;
  openBackup: () => Promise<Backup>;
  onBackupOpened: (backup: Backup) => boolean | Promise<boolean>;
  startSession: () => Promise<Session>;
}): Promise<Session | null> {
  const pasteTarget = steps.lockPasteTarget();
  // Its failure is reported below, unless capture fails first.
  pasteTarget.catch(() => undefined);
  const capture = await steps.startCapture();
  await steps.onCaptureStarted(capture);
  // The backup opens while the paste target is still being locked. It is
  // handed over first, so a paste target failure still finishes it.
  const proceed = await steps.onBackupOpened(await steps.openBackup());
  // A cancelled recording neither waits for nor reports the paste target.
  if (!proceed) return null;
  await pasteTarget;
  return steps.startSession();
}

/**
 * Keeps audio captured before a session exists, then uploads every chunk
 * serially. Serial uploads make the byte order independent of IPC latency.
 * Each recording gets its own queue, so one's uploads never reach another.
 */
export class SessionAudioQueue {
  private sessionId: string | null = null;
  private buffered: ArrayBuffer[] = [];
  private uploadTail: Promise<void> = Promise.resolve();
  private firstError: unknown = null;

  constructor(private readonly upload: AudioChunkUploader) {}

  append(buffer: ArrayBuffer): void {
    if (buffer.byteLength === 0) return;
    if (!this.sessionId) {
      this.buffered.push(buffer);
      return;
    }
    this.scheduleUpload(this.sessionId, buffer);
  }

  attachSession(sessionId: string): void {
    if (this.sessionId && this.sessionId !== sessionId) {
      throw new Error("An audio queue cannot be attached to two sessions");
    }
    this.sessionId = sessionId;
    const openingAudio = this.buffered.splice(0);
    for (const buffer of openingAudio) this.scheduleUpload(sessionId, buffer);
  }

  async drain(): Promise<void> {
    await this.uploadTail;
    if (this.firstError !== null) throw this.firstError;
  }

  private scheduleUpload(sessionId: string, buffer: ArrayBuffer): void {
    this.uploadTail = this.uploadTail
      .then(async () => {
        await this.upload(sessionId, buffer);
      })
      .catch((error) => {
        if (this.firstError === null) this.firstError = error;
      });
  }
}
