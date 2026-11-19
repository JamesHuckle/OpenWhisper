// The recording meter: one analyser on the running capture's source and one
// animation loop that feeds both the scrolling bars and the silence check.
// It listens in the capture's own audio context: a context of its own opened
// the speakers, which woke Bluetooth headphones and froze the page for about
// a second on every recording.
export const METER_SLICES = 20;
export const SILENT_THRESHOLD = 14;
// The silence check reads the newest 256 samples of the analyser's window.
const SILENCE_WINDOW = 256;

export type MeterView = {
  // Bar values, oldest first: 255 minus each frame's sample range.
  draw: (history: readonly number[]) => void;
  isRecording: () => boolean;
  setSilent: (silent: boolean) => void;
};

export function sampleRange(samples: Uint8Array): number {
  let min = 255;
  let max = 0;
  for (const value of samples) {
    if (value < min) min = value;
    if (value > max) max = value;
  }
  return max - min;
}

// Starts the loop on the next frame; returns the function that stops it.
export function startMeter(source: AudioNode, view: MeterView): () => void {
  const analyser = source.context.createAnalyser();
  source.connect(analyser);
  const data = new Uint8Array(analyser.fftSize);
  // The bars read the oldest half of the default 2048-sample window.
  const bars = data.subarray(0, analyser.frequencyBinCount);
  const recent = data.subarray(-SILENCE_WINDOW);
  const history: number[] = [];

  function tick() {
    analyser.getByteTimeDomainData(data);
    history.push(255 - sampleRange(bars));
    if (history.length > METER_SLICES) history.shift();
    view.draw(history);
    if (view.isRecording()) view.setSilent(sampleRange(recent) < SILENT_THRESHOLD);
    frameId = requestAnimationFrame(tick);
  }
  let frameId = requestAnimationFrame(tick);

  return () => {
    cancelAnimationFrame(frameId);
    try {
      source.disconnect(analyser);
    } catch {
      // Stopping the capture already disconnected the source.
    }
  };
}
