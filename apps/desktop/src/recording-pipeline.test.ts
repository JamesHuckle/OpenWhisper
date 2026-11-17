import { describe, expect, it } from "vitest";
import {
  joinAudioParts,
  sendAudioChunk,
  SessionAudioQueue,
  startCaptureFirst,
  type Invoke,
} from "./recording-pipeline";

function chunk(value: number): ArrayBuffer {
  return Uint8Array.of(value).buffer;
}

function valueOf(buffer: ArrayBuffer): number {
  return new Uint8Array(buffer)[0];
}

type FakeInvoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;

// Builds the startup steps the way dictation-controller.ts does, over a fake Tauri invoke.
function microphoneStartup(invoke: FakeInvoke, events: string[], cancelled = () => false) {
  return startCaptureFirst({
    lockPasteTarget: () => invoke("capture_paste_target"),
    startCapture: async () => {
      events.push("capture");
      return "microphone";
    },
    onCaptureStarted: async () => {
      events.push("indicator");
    },
    openBackup: () => invoke("recording_begin") as Promise<{ id: string }>,
    onBackupOpened: (backup) => {
      events.push(`backup ${backup.id}`);
      return !cancelled();
    },
    startSession: () => invoke("worker_start_session") as Promise<string>,
  });
}

async function settle() {
  for (let i = 0; i < 10; i++) await Promise.resolve();
}

describe("recording startup latency contracts", () => {
  it("starts capture before a slow recording_begin finishes and holds the session until it does", async () => {
    const events: string[] = [];
    const late = new Map<string, (value: unknown) => void>();
    const invoke: FakeInvoke = (command) => {
      events.push(`invoke ${command}`);
      return new Promise((resolve) => late.set(command, resolve));
    };

    const started = microphoneStartup(invoke, events);
    await settle();

    // Neither the paste target nor the backup file has answered, yet the
    // microphone is capturing and the indicator is showing.
    expect(events).toEqual([
      "invoke capture_paste_target",
      "capture",
      "indicator",
      "invoke recording_begin",
    ]);

    late.get("recording_begin")!({ id: "rec-1" });
    await settle();
    expect(events.at(-1)).toBe("backup rec-1");
    expect(events).not.toContain("invoke worker_start_session");

    late.get("capture_paste_target")!(undefined);
    await settle();
    expect(events.at(-1)).toBe("invoke worker_start_session");
    late.get("worker_start_session")!("session-id");
    await expect(started).resolves.toBe("session-id");
  });

  it("starts no session for a recording cancelled while its backup opened", async () => {
    const events: string[] = [];
    const invoke: FakeInvoke = async (command) => {
      events.push(`invoke ${command}`);
      return command === "recording_begin" ? { id: "rec-2" } : undefined;
    };

    await expect(microphoneStartup(invoke, events, () => true)).resolves.toBeNull();
    expect(events).toContain("backup rec-2");
    expect(events).not.toContain("invoke worker_start_session");
  });

  it("returns null for a cancelled recording without waiting for the paste target", async () => {
    const events: string[] = [];
    const invoke: FakeInvoke = (command) => {
      events.push(`invoke ${command}`);
      if (command === "capture_paste_target") return new Promise(() => undefined);
      return Promise.resolve({ id: "rec-4" });
    };

    await expect(microphoneStartup(invoke, events, () => true)).resolves.toBeNull();
    expect(events).not.toContain("invoke worker_start_session");
  });

  it("hands over an opened backup before reporting a paste target failure", async () => {
    const events: string[] = [];
    const invoke: FakeInvoke = async (command) => {
      events.push(`invoke ${command}`);
      if (command === "capture_paste_target") throw new Error("no target");
      return { id: "rec-3" };
    };

    await expect(microphoneStartup(invoke, events)).rejects.toThrow("no target");
    expect(events).toContain("backup rec-3");
    expect(events).not.toContain("invoke worker_start_session");
  });

  it("never lets cold worker latency postpone capture or the recording indicator", async () => {
    const events: string[] = [];
    let releaseWorker!: (session: string) => void;
    const workerStartup = new Promise<string>((resolve) => {
      releaseWorker = resolve;
    });

    const started = startCaptureFirst({
      lockPasteTarget: async () => undefined,
      startCapture: async () => {
        events.push("capture");
        return "microphone";
      },
      onCaptureStarted: async () => {
        events.push("indicator");
      },
      openBackup: async () => "backup",
      onBackupOpened: () => true,
      startSession: async () => {
        events.push("worker");
        return workerStartup;
      },
    });
    await settle();

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

  it("sends batched audio to the shell as the same raw bytes, in order", async () => {
    const calls: { command: string; bytes: number[]; headers: unknown }[] = [];
    let releaseFirst!: () => void;
    const firstGate = new Promise<void>((resolve) => {
      releaseFirst = resolve;
    });
    const invoke: Invoke = async (command, args, options) => {
      // Raw bytes, not a JSON object holding base64 text.
      expect(args instanceof Uint8Array || args instanceof ArrayBuffer).toBe(true);
      const bytes = Array.from(new Uint8Array(args as ArrayBuffer | Uint8Array));
      // The first batch is slow to save; the second still waits for it.
      if (bytes[0] === 1) await firstGate;
      calls.push({ command, bytes, headers: options?.headers });
    };

    // The backup gets each batch, joined from the captured parts, in order.
    let backupTail = Promise.resolve();
    for (const parts of [[Uint8Array.of(1, 2), Uint8Array.of(3)], [Uint8Array.of(4, 255)]]) {
      const combined = joinAudioParts(parts);
      backupTail = backupTail.then(() =>
        sendAudioChunk(invoke, "recording_append_chunk", "recording-1", combined),
      );
    }
    // A Live session gets the same batches through its queue.
    const queue = new SessionAudioQueue((sessionId, buffer) =>
      sendAudioChunk(invoke, "worker_append_audio_chunk", sessionId, buffer),
    );
    queue.append(Uint8Array.of(5, 6).buffer);
    queue.attachSession("session-1");
    queue.append(Uint8Array.of(7).buffer);

    await settle();
    releaseFirst();
    await backupTail;
    await queue.drain();

    const backup = calls.filter((call) => call.command === "recording_append_chunk");
    const live = calls.filter((call) => call.command === "worker_append_audio_chunk");
    expect(backup).toEqual([
      { command: "recording_append_chunk", bytes: [1, 2, 3], headers: { "recording-id": "recording-1" } },
      { command: "recording_append_chunk", bytes: [4, 255], headers: { "recording-id": "recording-1" } },
    ]);
    expect(live).toEqual([
      { command: "worker_append_audio_chunk", bytes: [5, 6], headers: { "session-id": "session-1" } },
      { command: "worker_append_audio_chunk", bytes: [7], headers: { "session-id": "session-1" } },
    ]);
  });
});
