// The page's microphone capture: opens the picked microphone and runs it
// through the PCM worklet (pcm-capture.worklet.ts) in one audio context,
// built once and kept suspended between recordings. main.ts wires it to the
// dictation flow, the level meter and the microphone menu.
import type { AudioCapture } from "./dictation-controller";
import { errorMessage } from "./error-message";
import {
  PCM_FLUSH_REQUEST,
  PCM_FLUSHED,
  PCM_SAMPLE_RATE,
  PCM_WORKLET_NAME,
} from "./pcm-capture";
import pcmCaptureWorkletUrl from "./pcm-capture.worklet?worker&url";

export type MicrophoneCaptureDeps = {
  /** The picked microphone's device id, or "default" for the system default. */
  selectedMicrophoneId: () => string;
  /** The picked microphone was gone, so the system default was opened and is now the choice. */
  onFellBackToDefaultMicrophone: () => void;
  log: (message: string) => void;
};

export type MicrophoneCapture = AudioCapture & {
  /** The running capture's microphone node, for the level meter; null when none runs. */
  source: () => MediaStreamAudioSourceNode | null;
  /** Builds the audio context before the first recording; a failure is retried then. */
  warmUp: () => void;
};

// The context runs at the upload rate, so the browser's filtered resampler
// converts the microphone audio. The worklet only packs it as 16-bit PCM, on
// the audio thread, so a busy page cannot drop samples.
//
// Capture never plays sound, so the context opens no speaker ("none" sink).
// Opening the default speaker woke Bluetooth headphones on every recording,
// which held up the microphone by a second or more. Runtimes without this
// option ignore it and use the default speaker as before.
const CAPTURE_CONTEXT_OPTIONS = {
  sampleRate: PCM_SAMPLE_RATE,
  sinkId: { type: "none" },
} as AudioContextOptions & { sinkId: { type: "none" } };

export function createMicrophoneCapture(deps: MicrophoneCaptureDeps): MicrophoneCapture {
  // Built once at the upload rate with the worklet loaded, then suspended
  // between recordings, so the microphone opens without rebuilding it.
  let pcmContextReady: Promise<AudioContext> | null = null;
  // The context of the capture now running; null when none is.
  let pcmAudioContext: AudioContext | null = null;
  let pcmSource: MediaStreamAudioSourceNode | null = null;
  let pcmProcessor: AudioWorkletNode | null = null;
  let pcmSilentGain: GainNode | null = null;
  // Where the running capture sends its audio.
  let pcmOnAudio: ((chunk: ArrayBuffer) => void) | null = null;
  let micStream: MediaStream | null = null;

  // Opens the picked microphone. One that is gone (headphones switched off or
  // unplugged) falls back to the Windows default, which becomes the choice.
  async function openMicrophone(): Promise<MediaStream> {
    const selectedMicId = deps.selectedMicrophoneId();
    const constraints: MediaStreamConstraints = {
      audio: selectedMicId === "default" ? true : { deviceId: { exact: selectedMicId } },
    };
    try {
      return await navigator.mediaDevices.getUserMedia(constraints);
    } catch (error) {
      const name = error instanceof DOMException ? error.name : "";
      const deviceGone = name === "OverconstrainedError" || name === "NotFoundError";
      if (selectedMicId === "default" || !deviceGone) throw error;
      deps.log(`picked microphone unavailable (${name}); using the system default`);
      deps.onFellBackToDefaultMicrophone();
      return navigator.mediaDevices.getUserMedia({ audio: true });
    }
  }

  function pcmCaptureContext(): Promise<AudioContext> {
    if (!pcmContextReady) {
      const ready = (async () => {
        const context = new AudioContext(CAPTURE_CONTEXT_OPTIONS);
        try {
          if (context.sampleRate !== PCM_SAMPLE_RATE) {
            throw new Error(
              `Audio capture runs at ${context.sampleRate} Hz, not ${PCM_SAMPLE_RATE} Hz`,
            );
          }
          await context.audioWorklet.addModule(pcmCaptureWorkletUrl);
          await context.suspend();
          return context;
        } catch (error) {
          void context.close();
          throw error;
        }
      })();
      pcmContextReady = ready;
      // A failed build is tried again at the next recording.
      ready.catch(() => {
        if (pcmContextReady === ready) pcmContextReady = null;
      });
    }
    return pcmContextReady;
  }

  async function startPcmCapture(stream: MediaStream, onAudio: (chunk: ArrayBuffer) => void) {
    let context = await pcmCaptureContext();
    if (context.state === "closed") {
      pcmContextReady = null;
      context = await pcmCaptureContext();
    }
    await context.resume();
    const source = context.createMediaStreamSource(stream);
    let processor: AudioWorkletNode;
    try {
      processor = new AudioWorkletNode(context, PCM_WORKLET_NAME, {
        numberOfInputs: 1,
        numberOfOutputs: 1,
        outputChannelCount: [1],
        channelCount: 1,
        channelCountMode: "explicit",
        channelInterpretation: "speakers",
      });
    } catch (error) {
      void context.suspend();
      throw error;
    }
    processor.port.onmessage = (event: MessageEvent<ArrayBuffer | string>) => {
      if (event.data instanceof ArrayBuffer) onAudio(event.data);
    };
    // A silent path to the speakers keeps the browser pulling audio through
    // the worklet.
    const silentGain = context.createGain();
    silentGain.gain.value = 0;
    source.connect(processor);
    processor.connect(silentGain);
    silentGain.connect(context.destination);
    pcmAudioContext = context;
    pcmSource = source;
    pcmProcessor = processor;
    pcmSilentGain = silentGain;
    pcmOnAudio = onAudio;
  }

  // Asks the worklet for the samples that did not yet fill a chunk, so the end
  // of the recording is kept. Gives up after a short wait.
  function flushPcmWorklet(
    processor: AudioWorkletNode,
    onAudio: (chunk: ArrayBuffer) => void,
  ): Promise<void> {
    return new Promise((resolve) => {
      const timeout = setTimeout(resolve, 250);
      processor.port.onmessage = (event: MessageEvent<ArrayBuffer | string>) => {
        if (event.data instanceof ArrayBuffer) {
          onAudio(event.data);
        } else if (event.data === PCM_FLUSHED) {
          clearTimeout(timeout);
          resolve();
        }
      };
      processor.port.postMessage(PCM_FLUSH_REQUEST);
    });
  }

  // keepTail is false when the recording is being thrown away.
  async function stopPcmCapture(keepTail = true) {
    const processor = pcmProcessor;
    const onAudio = pcmOnAudio;
    if (processor && onAudio && keepTail && pcmAudioContext?.state === "running") {
      await flushPcmWorklet(processor, onAudio);
      // Another stop finished this capture while the tail was on its way.
      if (pcmProcessor !== processor) return;
    }
    if (processor) processor.port.onmessage = null;
    pcmSource?.disconnect();
    pcmProcessor?.disconnect();
    pcmSilentGain?.disconnect();
    pcmSource = null;
    pcmProcessor = null;
    pcmSilentGain = null;
    pcmOnAudio = null;
    const context = pcmAudioContext;
    pcmAudioContext = null;
    micStream?.getTracks().forEach((track) => track.stop());
    // Kept for the next recording; suspended, it does no work meanwhile. One
    // the system closed is rebuilt when the next recording starts.
    if (context) await context.suspend().catch(() => undefined);
  }

  return {
    async start(onAudio) {
      micStream = await openMicrophone();
      await startPcmCapture(micStream, onAudio);
    },
    isActive: () => pcmAudioContext !== null,
    stop: () => stopPcmCapture(),
    discard() {
      if (pcmAudioContext) void stopPcmCapture(false);
      micStream?.getTracks().forEach((track) => track.stop());
      micStream = null;
    },
    source: () => pcmSource,
    warmUp() {
      void pcmCaptureContext().catch((error) => {
        deps.log(`audio capture warm-up failed=${errorMessage(error)}`);
      });
    },
  };
}
