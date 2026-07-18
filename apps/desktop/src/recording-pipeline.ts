export type AudioChunkUploader = (sessionId: string, buffer: ArrayBuffer) => Promise<unknown>;

export async function captureBeforeSession<Capture, Session>(steps: {
  startCapture: () => Promise<Capture>;
  onCaptureStarted: (capture: Capture) => void | Promise<void>;
  startSession: () => Promise<Session>;
}): Promise<Session> {
  const capture = await steps.startCapture();
  await steps.onCaptureStarted(capture);
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
