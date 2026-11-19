import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { METER_SLICES, SILENT_THRESHOLD, sampleRange, startMeter } from "./meter";

// A fake analyser with the browser's default 2048-sample window. Each frame
// fills it from `frame`: the bars must read only the oldest 1024 samples and
// the silence check only the newest 256.
function fakeSource() {
  let frame = { bars: [128, 128], middle: [128, 128], recent: [128, 128] };
  const analyser = {
    fftSize: 2048,
    frequencyBinCount: 1024,
    getByteTimeDomainData(data: Uint8Array) {
      data.fill(frame.bars[0], 0, 1024);
      data[1] = frame.bars[1];
      data.fill(frame.middle[0], 1024, 1792);
      data[1500] = frame.middle[1];
      data.fill(frame.recent[0], 1792);
      data[2000] = frame.recent[1];
    },
  };
  const source = {
    context: { createAnalyser: vi.fn(() => analyser) },
    connect: vi.fn(),
    disconnect: vi.fn(),
  };
  return {
    source,
    analyser,
    setFrame(next: Partial<typeof frame>) {
      frame = { ...frame, ...next };
    },
  };
}

let frames: Map<number, FrameRequestCallback>;
let nextFrameId: number;

function runFrame() {
  const pending = [...frames.values()];
  frames.clear();
  pending.forEach((callback) => callback(0));
}

beforeEach(() => {
  frames = new Map();
  nextFrameId = 1;
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    frames.set(nextFrameId, callback);
    return nextFrameId++;
  });
  vi.stubGlobal("cancelAnimationFrame", (id: number) => frames.delete(id));
});

afterEach(() => {
  vi.unstubAllGlobals();
});

function start(recording = () => true) {
  const fake = fakeSource();
  const drawn: number[][] = [];
  const silent: boolean[] = [];
  const stop = startMeter(fake.source as unknown as AudioNode, {
    draw: (history) => drawn.push([...history]),
    isRecording: recording,
    setSilent: (value) => silent.push(value),
  });
  return { ...fake, drawn, silent, stop };
}

describe("recording meter", () => {
  it("measures the spread between the lowest and highest sample", () => {
    expect(sampleRange(Uint8Array.from([128, 140, 100, 128]))).toBe(40);
    expect(sampleRange(Uint8Array.from([128, 128]))).toBe(0);
  });

  it("runs one analyser and one loop for both the bars and the silence check", () => {
    const meter = start();

    expect(meter.source.context.createAnalyser).toHaveBeenCalledTimes(1);
    expect(meter.source.connect).toHaveBeenCalledWith(meter.analyser);
    expect(frames.size).toBe(1);
    expect(meter.drawn).toEqual([]);

    runFrame();
    expect(frames.size).toBe(1);
    expect(meter.drawn).toHaveLength(1);
    expect(meter.silent).toHaveLength(1);
  });

  it("draws 255 minus the range of the oldest 1024 samples, newest bar last", () => {
    const meter = start();

    meter.setFrame({ bars: [100, 160], middle: [0, 255], recent: [128, 128] });
    runFrame();
    meter.setFrame({ bars: [128, 128] });
    runFrame();

    expect(meter.drawn).toEqual([[195], [195, 255]]);
  });

  it("keeps the last 20 bars", () => {
    const meter = start();

    for (let index = 0; index < METER_SLICES + 5; index++) {
      meter.setFrame({ bars: [128, 128 + index] });
      runFrame();
    }

    const last = meter.drawn.at(-1)!;
    expect(last).toHaveLength(METER_SLICES);
    expect(last[0]).toBe(255 - 5);
    expect(last.at(-1)).toBe(255 - (METER_SLICES + 4));
  });

  it("calls the newest 256 samples silent below the threshold", () => {
    const meter = start();

    meter.setFrame({ bars: [0, 255], middle: [0, 255], recent: [128, 128 + SILENT_THRESHOLD - 1] });
    runFrame();
    meter.setFrame({ bars: [128, 128], middle: [128, 128], recent: [128, 128 + SILENT_THRESHOLD] });
    runFrame();

    expect(meter.silent).toEqual([true, false]);
  });

  it("keeps drawing but leaves the silence alone once recording ends", () => {
    let recording = true;
    const meter = start(() => recording);

    runFrame();
    recording = false;
    runFrame();
    runFrame();

    expect(meter.drawn).toHaveLength(3);
    expect(meter.silent).toEqual([true]);
  });

  it("stops the loop and disconnects the analyser", () => {
    const meter = start();
    runFrame();

    meter.stop();
    runFrame();

    expect(frames.size).toBe(0);
    expect(meter.drawn).toHaveLength(1);
    expect(meter.source.disconnect).toHaveBeenCalledWith(meter.analyser);
  });

  it("stops quietly when stopping the capture already disconnected the source", () => {
    const meter = start();
    meter.source.disconnect.mockImplementation(() => {
      throw new Error("InvalidAccessError");
    });

    expect(() => meter.stop()).not.toThrow();
    expect(frames.size).toBe(0);
  });
});
