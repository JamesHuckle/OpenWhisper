import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createMicrophoneCapture } from "./microphone-capture";
import { PCM_FLUSH_REQUEST, PCM_FLUSHED, PCM_SAMPLE_RATE } from "./pcm-capture";

// Runs the page's microphone capture against small stand-ins for the browser's
// microphone, audio context and worklet node. `events` records, in order,
// what reaches the page and what is torn down.
let events: string[];
let contexts: FakeAudioContext[];
let workletNodes: FakeAudioWorkletNode[];
let failNextWorkletLoad: boolean;
// What the worklet does when the page asks for the tail: reply with it, or
// never answer (a stalled audio thread).
let workletAnswersFlush: boolean;
const getUserMedia = vi.fn();

const audioNode = (name: string) => ({
  connect: vi.fn(),
  disconnect: vi.fn(() => events.push(`${name} disconnected`)),
});

class FakeAudioContext {
  state: AudioContextState = "running";
  sampleRate: number;
  destination = audioNode("speaker");
  audioWorklet = {
    addModule: vi.fn(async () => {
      if (failNextWorkletLoad) {
        failNextWorkletLoad = false;
        throw new Error("worklet did not load");
      }
    }),
  };
  constructor(readonly options: AudioContextOptions) {
    this.sampleRate = options.sampleRate!;
    contexts.push(this);
  }
  resume = async () => void (this.state = "running");
  suspend = async () => void (this.state = "suspended");
  close = async () => void (this.state = "closed");
  createMediaStreamSource = () => audioNode("source");
  createGain = () => ({ ...audioNode("gain"), gain: { value: 1 } });
}

class FakeAudioWorkletNode {
  connect = vi.fn();
  disconnect = vi.fn(() => events.push("worklet disconnected"));
  port = {
    onmessage: null as ((event: { data: ArrayBuffer | string }) => void) | null,
    postMessage: (message: string) => {
      events.push(`asked for ${message}`);
      if (message !== PCM_FLUSH_REQUEST || !workletAnswersFlush) return;
      this.port.onmessage?.({ data: new ArrayBuffer(512) });
      this.port.onmessage?.({ data: PCM_FLUSHED });
    },
  };
  constructor() {
    workletNodes.push(this);
  }
}

function microphone() {
  return { getTracks: () => [{ stop: () => events.push("microphone closed") }] };
}

function setup(selectedMicrophoneId = "default") {
  const deps = {
    selectedMicrophoneId: () => selectedMicrophoneId,
    onFellBackToDefaultMicrophone: vi.fn(() => {
      selectedMicrophoneId = "default";
    }),
    log: vi.fn(),
  };
  const onAudio = (chunk: ArrayBuffer) => events.push(`audio ${chunk.byteLength} bytes`);
  return { capture: createMicrophoneCapture(deps), deps, onAudio };
}

beforeEach(() => {
  events = [];
  contexts = [];
  workletNodes = [];
  failNextWorkletLoad = false;
  workletAnswersFlush = true;
  getUserMedia.mockReset().mockImplementation(async () => microphone());
  vi.stubGlobal("AudioContext", FakeAudioContext);
  vi.stubGlobal("AudioWorkletNode", FakeAudioWorkletNode);
  vi.stubGlobal("navigator", { mediaDevices: { getUserMedia } });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("microphone capture", () => {
  it("opens one context at the upload rate with no speaker, and keeps it between recordings", async () => {
    const { capture, onAudio } = setup("usb-mic");

    await capture.start(onAudio);
    await capture.stop();
    await capture.start(onAudio);

    expect(getUserMedia).toHaveBeenCalledWith({ audio: { deviceId: { exact: "usb-mic" } } });
    expect(contexts).toHaveLength(1);
    expect(contexts[0].options).toEqual({ sampleRate: PCM_SAMPLE_RATE, sinkId: { type: "none" } });
    expect(capture.isActive()).toBe(true);
    expect(capture.source()).not.toBeNull();
  });

  it.each(["OverconstrainedError", "NotFoundError"])(
    "falls back to the system default when the picked microphone is gone (%s)",
    async (errorName) => {
      // Bug (fixed in 70e22c6): a switched-off or unplugged microphone failed every recording.
      getUserMedia.mockRejectedValueOnce(new DOMException("gone", errorName));
      const { capture, deps, onAudio } = setup("usb-mic");

      await capture.start(onAudio);

      expect(getUserMedia.mock.calls).toEqual([
        [{ audio: { deviceId: { exact: "usb-mic" } } }],
        [{ audio: true }],
      ]);
      expect(deps.onFellBackToDefaultMicrophone).toHaveBeenCalledTimes(1);
      expect(deps.log).toHaveBeenCalledWith(`picked microphone unavailable (${errorName}); using the system default`);
      expect(capture.isActive()).toBe(true);
    },
  );

  it.each([
    ["a refused permission for the picked microphone", "usb-mic", "NotAllowedError"],
    ["a missing system default", "default", "NotFoundError"],
  ])("passes on %s without falling back", async (_case, microphoneId, errorName) => {
    const error = new DOMException("no", errorName);
    getUserMedia.mockRejectedValueOnce(error);
    const { capture, deps, onAudio } = setup(microphoneId);

    await expect(capture.start(onAudio)).rejects.toBe(error);

    expect(getUserMedia).toHaveBeenCalledTimes(1);
    expect(deps.onFellBackToDefaultMicrophone).not.toHaveBeenCalled();
    expect(capture.isActive()).toBe(false);
  });

  it("delivers the last audio before tearing the capture down", async () => {
    const { capture, onAudio } = setup();
    await capture.start(onAudio);

    await capture.stop();

    expect(events).toEqual([
      `asked for ${PCM_FLUSH_REQUEST}`,
      "audio 512 bytes",
      "source disconnected",
      "worklet disconnected",
      "gain disconnected",
      "microphone closed",
    ]);
    expect(capture.isActive()).toBe(false);
    expect(capture.source()).toBeNull();
    expect(contexts[0].state).toBe("suspended");
  });

  it("stops waiting for the last audio after 250 ms", async () => {
    vi.useFakeTimers();
    workletAnswersFlush = false;
    const { capture, onAudio } = setup();
    await capture.start(onAudio);

    const stopped = capture.stop();
    await vi.advanceTimersByTimeAsync(249);
    expect(events).toEqual([`asked for ${PCM_FLUSH_REQUEST}`]);
    expect(capture.isActive()).toBe(true);

    await vi.advanceTimersByTimeAsync(1);
    await stopped;
    expect(events).toContain("worklet disconnected");
    expect(capture.isActive()).toBe(false);
  });

  it("tears down once when a second stop comes while the last audio is awaited", async () => {
    vi.useFakeTimers();
    workletAnswersFlush = false;
    const { capture, onAudio } = setup();
    await capture.start(onAudio);

    const stops = Promise.all([capture.stop(), capture.stop()]);
    await vi.advanceTimersByTimeAsync(250);
    await stops;

    expect(events.filter((event) => event === "worklet disconnected")).toHaveLength(1);
    expect(events.filter((event) => event === "microphone closed")).toHaveLength(1);
  });

  it("drops the unsent audio when a recording is discarded", async () => {
    const { capture, onAudio } = setup();
    await capture.start(onAudio);

    capture.discard();
    await vi.waitFor(() => expect(contexts[0].state).toBe("suspended"));

    expect(events).not.toContain(`asked for ${PCM_FLUSH_REQUEST}`);
    expect(events).toContain("worklet disconnected");
    expect(events).toContain("microphone closed");
    expect(capture.isActive()).toBe(false);
  });

  it("rebuilds a context the system closed between recordings", async () => {
    const { capture, onAudio } = setup();
    await capture.start(onAudio);
    await capture.stop();
    await contexts[0].close();

    await capture.start(onAudio);

    expect(contexts).toHaveLength(2);
    expect(contexts[1].state).toBe("running");
    expect(workletNodes).toHaveLength(2);
    expect(capture.isActive()).toBe(true);
  });

  it("builds the context again at the next recording when the warm-up failed", async () => {
    failNextWorkletLoad = true;
    const { capture, deps, onAudio } = setup();

    capture.warmUp();
    await vi.waitFor(() =>
      expect(deps.log).toHaveBeenCalledWith("audio capture warm-up failed=worklet did not load"),
    );
    expect(contexts[0].state).toBe("closed");

    await capture.start(onAudio);

    expect(contexts).toHaveLength(2);
    expect(contexts[1].state).toBe("running");
    expect(capture.isActive()).toBe(true);
  });
});
