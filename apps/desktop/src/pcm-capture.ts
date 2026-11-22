// Microphone capture format shared by the page (main.ts) and the audio
// worklet (pcm-capture.worklet.ts).
//
// The page opens its AudioContext at PCM_SAMPLE_RATE, so the browser converts
// the microphone's own rate (usually 44.1 or 48 kHz) with its filtered
// resampler. Nothing here changes the sample rate: it only turns the float
// samples into 16-bit little-endian PCM in fixed-size chunks.

export const PCM_SAMPLE_RATE = 24_000;
export const PCM_WORKLET_NAME = "openwhisper-pcm-capture";
/** Frames per chunk sent to the page: about 85 ms at 24 kHz. */
export const PCM_CHUNK_FRAMES = 2048;
/** Page-to-worklet message asking for the samples not yet sent. */
export const PCM_FLUSH_REQUEST = "flush";
/** Worklet-to-page message sent after the last flushed chunk. */
export const PCM_FLUSHED = "flushed";

export function floatToPcm16(input: Float32Array): ArrayBuffer {
  const buffer = new ArrayBuffer(input.length * 2);
  const view = new DataView(buffer);
  for (let i = 0; i < input.length; i++) {
    const sample = Math.max(-1, Math.min(1, input[i]));
    view.setInt16(i * 2, sample < 0 ? sample * 0x8000 : sample * 0x7fff, true);
  }
  return buffer;
}

/**
 * Collects the audio thread's small render blocks (128 frames) into
 * PCM_CHUNK_FRAMES-sized PCM16 chunks, so the page gets a few messages a
 * second instead of hundreds.
 */
export class Pcm16Chunker {
  private readonly pending: Float32Array;
  private filled = 0;

  constructor(private readonly chunkFrames = PCM_CHUNK_FRAMES) {
    this.pending = new Float32Array(chunkFrames);
  }

  /** Adds samples and returns every chunk that is now full. */
  push(samples: Float32Array): ArrayBuffer[] {
    const chunks: ArrayBuffer[] = [];
    let offset = 0;
    while (offset < samples.length) {
      const count = Math.min(this.chunkFrames - this.filled, samples.length - offset);
      this.pending.set(samples.subarray(offset, offset + count), this.filled);
      this.filled += count;
      offset += count;
      if (this.filled === this.chunkFrames) {
        chunks.push(floatToPcm16(this.pending));
        this.filled = 0;
      }
    }
    return chunks;
  }

  /** Returns the samples held back from the last partial chunk, if any. */
  flush(): ArrayBuffer | null {
    if (this.filled === 0) return null;
    const chunk = floatToPcm16(this.pending.subarray(0, this.filled));
    this.filled = 0;
    return chunk;
  }
}
