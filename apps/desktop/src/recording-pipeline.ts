export type AudioChunkUploader = (sessionId: string, buffer: ArrayBuffer) => Promise<unknown>;

/**
 * Starts a recording with the microphone first, so words said right after the
 * shortcut are kept. The paste target is locked at once, alongside capture;
 * the backup file opens once capture runs, and the worker session starts
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
 * Keeps audio captured before a worker session exists, then uploads every chunk
 * serially. Serial uploads make the byte order independent of IPC latency.
 */
export class SessionAudioQueue {
  private sessionId: string | null = null;
  private buffered: ArrayBuffer[] = [];
  private ingestTail: Promise<void> = Promise.resolve();
  private uploadTail: Promise<void> = Promise.resolve();
  private firstError: unknown = null;
  private generation = 0;

  constructor(private readonly upload: AudioChunkUploader) {}

  get bufferedChunkCount(): number {
    return this.buffered.length;
  }

  append(buffer: ArrayBuffer): void {
    if (buffer.byteLength === 0) return;
    if (!this.sessionId) {
      this.buffered.push(buffer);
      return;
    }
    this.scheduleUpload(this.sessionId, buffer);
  }

  appendAsync(buffer: Promise<ArrayBuffer>): void {
    const generation = this.generation;
    this.ingestTail = this.ingestTail
      .then(async () => {
        try {
          const resolved = await buffer;
          if (generation === this.generation) this.append(resolved);
        } catch (error) {
          if (generation === this.generation) this.rememberError(error);
        }
      });
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
    await this.ingestTail;
    await this.uploadTail;
    if (this.firstError !== null) throw this.firstError;
  }

  reset(): void {
    this.generation += 1;
    this.sessionId = null;
    this.buffered = [];
    this.ingestTail = Promise.resolve();
    this.uploadTail = Promise.resolve();
    this.firstError = null;
  }

  private scheduleUpload(sessionId: string, buffer: ArrayBuffer): void {
    const generation = this.generation;
    this.uploadTail = this.uploadTail
      .then(async () => {
        await this.upload(sessionId, buffer);
      })
      .catch((error) => {
        if (generation === this.generation) this.rememberError(error);
      });
  }

  private rememberError(error: unknown): void {
    if (this.firstError === null) this.firstError = error;
  }
}
