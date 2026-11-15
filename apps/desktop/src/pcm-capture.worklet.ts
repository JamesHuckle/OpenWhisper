// Runs on the browser's audio thread, so a busy page cannot make it skip
// microphone audio. main.ts loads it with `?worker&url`, which makes Vite
// bundle it (with its import) into one script for audioWorklet.addModule.
import {
  PCM_FLUSH_REQUEST,
  PCM_FLUSHED,
  PCM_WORKLET_NAME,
  Pcm16Chunker,
} from "./pcm-capture";

// The audio-thread globals are not in TypeScript's DOM library.
declare class AudioWorkletProcessor {
  readonly port: MessagePort;
}
declare function registerProcessor(
  name: string,
  processorCtor: new () => AudioWorkletProcessor,
): void;

export class PcmCaptureProcessor extends AudioWorkletProcessor {
  private readonly chunker = new Pcm16Chunker();

  constructor() {
    super();
    this.port.onmessage = (event: MessageEvent) => {
      if (event.data !== PCM_FLUSH_REQUEST) return;
      const tail = this.chunker.flush();
      if (tail) this.port.postMessage(tail, [tail]);
      this.port.postMessage(PCM_FLUSHED);
    };
  }

  process(inputs: Float32Array[][]): boolean {
    // Mono: the node is created with one input channel. An input with no
    // channels means the microphone is not connected yet; keep running.
    const samples = inputs[0]?.[0];
    if (samples) {
      for (const chunk of this.chunker.push(samples)) {
        this.port.postMessage(chunk, [chunk]);
      }
    }
    return true;
  }
}

registerProcessor(PCM_WORKLET_NAME, PcmCaptureProcessor);
