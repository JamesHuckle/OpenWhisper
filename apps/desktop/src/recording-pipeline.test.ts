import { describe, expect, it } from "vitest";
import { captureBeforeSession, SessionAudioQueue } from "./recording-pipeline";

function chunk(value: number): ArrayBuffer {
  return Uint8Array.of(value).buffer;
}

function valueOf(buffer: ArrayBuffer): number {
  return new Uint8Array(buffer)[0];
}

describe("recording startup latency contracts", () => {
  it("never lets cold worker latency postpone capture or the recording indicator", async () => {
    const events: string[] = [];
    let releaseWorker!: (session: string) => void;
    const workerStartup = new Promise<string>((resolve) => {
      releaseWorker = resolve;
    });

    const started = captureBeforeSession({
      startCapture: async () => {
        events.push("capture");
        return "microphone";
      },
      onCaptureStarted: async () => {
        events.push("indicator");
      },
      startSession: async () => {
        events.push("worker");
        return workerStartup;
      },
    });
    await Promise.resolve();
    await Promise.resolve();

    expect(events).toEqual(["capture", "indicator", "worker"]);
    releaseWorker("session-id");
    await expect(started).resolves.toBe("session-id");
  });

  it("retains opening audio while worker startup is arbitrarily delayed", async () => {
    const uploaded: number[] = [];
    const queue = new SessionAudioQueue(async (_sessionId, buffer) => {
      uploaded.push(valueOf(buffer));
    });

    queue.append(chunk(1));
    queue.append(chunk(2));

    expect(uploaded).toEqual([]);
    expect(queue.bufferedChunkCount).toBe(2);

    queue.attachSession("cold-worker");
    await queue.drain();

    expect(uploaded).toEqual([1, 2]);
  });

  it("preserves capture order across slow IPC and chunks arriving after startup", async () => {
    const uploaded: number[] = [];
    let releaseFirstUpload!: () => void;
    const firstUploadGate = new Promise<void>((resolve) => {
      releaseFirstUpload = resolve;
    });
    const queue = new SessionAudioQueue(async (_sessionId, buffer) => {
      const value = valueOf(buffer);
      if (value === 1) await firstUploadGate;
      uploaded.push(value);
    });

    queue.append(chunk(1));
    queue.append(chunk(2));
    queue.attachSession("realtime-websocket");
    queue.append(chunk(3));
    await Promise.resolve();

    expect(uploaded).toEqual([]);
    releaseFirstUpload();
    await queue.drain();

    expect(uploaded).toEqual([1, 2, 3]);
  });

  it("waits for a final MediaRecorder blob conversion before finalization", async () => {
    const uploaded: number[] = [];
    let releaseConversion!: (buffer: ArrayBuffer) => void;
    const conversion = new Promise<ArrayBuffer>((resolve) => {
      releaseConversion = resolve;
    });
    const queue = new SessionAudioQueue(async (_sessionId, buffer) => {
      uploaded.push(valueOf(buffer));
    });

    queue.attachSession("release-during-startup");
    queue.appendAsync(conversion);
    const drained = queue.drain();
    await Promise.resolve();
    expect(uploaded).toEqual([]);

    releaseConversion(chunk(9));
    await drained;
    expect(uploaded).toEqual([9]);
  });

  it("cannot leak a stale blob conversion into the next recording", async () => {
    const uploaded: number[] = [];
    let releaseOldConversion!: (buffer: ArrayBuffer) => void;
    const oldConversion = new Promise<ArrayBuffer>((resolve) => {
      releaseOldConversion = resolve;
    });
    const queue = new SessionAudioQueue(async (_sessionId, buffer) => {
      uploaded.push(valueOf(buffer));
    });

    queue.appendAsync(oldConversion);
    queue.reset();
    queue.attachSession("next-recording");
    releaseOldConversion(chunk(4));
    await Promise.resolve();
    await Promise.resolve();
    await queue.drain();

    expect(uploaded).toEqual([]);
  });
});
