import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import {
  floatToPcm16,
  PCM_CHUNK_FRAMES,
  PCM_FLUSH_REQUEST,
  PCM_FLUSHED,
  PCM_MIME_TYPE,
  PCM_SAMPLE_RATE,
  PCM_WORKLET_NAME,
  Pcm16Chunker,
} from "./pcm-capture";

function int16(buffer: ArrayBuffer): number[] {
  return Array.from(new Int16Array(buffer));
}

function ramp(length: number, start = 0): Float32Array {
  return Float32Array.from({ length }, (_, i) => Math.sin((start + i) / 7) * 0.8);
}

describe("PCM16 conversion", () => {
  it("writes little-endian 16-bit samples and clips out-of-range input", () => {
    const bytes = new Uint8Array(floatToPcm16(Float32Array.of(0, 1, -1, 0.5, 2, -3)));

    expect(bytes.byteLength).toBe(12);
    expect(Array.from(bytes.subarray(2, 4))).toEqual([0xff, 0x7f]);
    expect(int16(bytes.buffer)).toEqual([0, 32767, -32768, 16383, 32767, -32768]);
  });

  it("keeps every sample: the page converts, it never changes the sample rate", () => {
    const input = ramp(4800);

    expect(new Int16Array(floatToPcm16(input)).length).toBe(input.length);
    expect(PCM_MIME_TYPE).toBe(`audio/pcm;rate=${PCM_SAMPLE_RATE}`);
    expect(PCM_SAMPLE_RATE).toBe(24_000);
  });
});

describe("PCM16 chunker", () => {
  it("joins 128-frame render blocks into full chunks without losing or reordering samples", () => {
    const chunker = new Pcm16Chunker();
    const source = ramp(PCM_CHUNK_FRAMES * 3 + 300);
    const chunks: ArrayBuffer[] = [];
    for (let i = 0; i < source.length; i += 128) {
      chunks.push(...chunker.push(source.subarray(i, i + 128)));
    }
    const tail = chunker.flush();

    expect(chunks).toHaveLength(3);
    expect(chunks.every((chunk) => chunk.byteLength === PCM_CHUNK_FRAMES * 2)).toBe(true);
    expect(tail?.byteLength).toBe(300 * 2);
    expect([...chunks, tail!].flatMap(int16)).toEqual(int16(floatToPcm16(source)));
    expect(chunker.flush()).toBeNull();
  });

  it("splits an input larger than one chunk", () => {
    const chunker = new Pcm16Chunker(4);
    const chunks = chunker.push(ramp(10));

    expect(chunks.map((chunk) => chunk.byteLength)).toEqual([8, 8]);
    expect(chunker.flush()?.byteLength).toBe(4);
  });
});

describe("PCM capture worklet", () => {
  async function loadProcessor() {
    const posted: { data: unknown; transfer?: Transferable[] }[] = [];
    const port = {
      onmessage: null as ((event: MessageEvent) => void) | null,
      postMessage(data: unknown, transfer?: Transferable[]) {
        posted.push({ data, transfer });
      },
    };
    let registered: { name: string; ctor: new () => { process(inputs: Float32Array[][]): boolean } } | null =
      null;
    const scope = globalThis as Record<string, unknown>;
    scope.AudioWorkletProcessor = class {
      readonly port = port;
    };
    scope.registerProcessor = (name: string, ctor: never) => {
      registered = { name, ctor };
    };
    try {
      await import("./pcm-capture.worklet");
    } finally {
      delete scope.AudioWorkletProcessor;
      delete scope.registerProcessor;
    }
    expect(registered).not.toBeNull();
    const { name, ctor } = registered!;
    return { name, processor: new ctor(), port, posted };
  }

  it("sends full chunks from the audio thread and the held-back tail when asked to flush", async () => {
    const { name, processor, port, posted } = await loadProcessor();
    const source = ramp(PCM_CHUNK_FRAMES + 256);

    expect(name).toBe(PCM_WORKLET_NAME);
    // No channels yet: the microphone is still connecting.
    expect(processor.process([[]])).toBe(true);
    for (let i = 0; i < source.length; i += 128) {
      expect(processor.process([[source.subarray(i, i + 128)]])).toBe(true);
    }
    expect(posted).toHaveLength(1);

    port.onmessage!({ data: PCM_FLUSH_REQUEST } as MessageEvent);

    expect(posted.map(({ data }) => (data instanceof ArrayBuffer ? data.byteLength : data))).toEqual([
      PCM_CHUNK_FRAMES * 2,
      256 * 2,
      PCM_FLUSHED,
    ]);
    // Chunks are moved, not copied, to the page.
    expect(posted[0].transfer).toEqual([posted[0].data]);
    expect(posted.slice(0, 2).flatMap(({ data }) => int16(data as ArrayBuffer))).toEqual(
      int16(floatToPcm16(source)),
    );
  });

  it("is opened by the page at the upload rate, with no speaker", () => {
    // Bugs: converting the rate here passed a 15 kHz tone through as a 9 kHz
    // whistle (checked in headless Chrome), and opening the default speaker
    // woke Bluetooth headphones and held up every recording. The page's one
    // context now runs at the upload rate with a "none" sink.
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");

    expect(main.match(/new AudioContext\([^)]*\)/g)).toEqual(["new AudioContext(CAPTURE_CONTEXT_OPTIONS)"]);
    expect(main).toContain("sampleRate: PCM_SAMPLE_RATE,\n  sinkId: { type: \"none\" },");
  });
});
