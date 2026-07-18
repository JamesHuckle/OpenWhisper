from __future__ import annotations

import argparse
import os
import time
import wave
from pathlib import Path

from openai import OpenAI

from openwhisper_worker.openai_client import RealtimeTranscriptionStream


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Prove that Realtime transcription deltas arrive before a WAV finishes uploading."
    )
    parser.add_argument("wav", type=Path, help="24 kHz mono PCM16 WAV file")
    parser.add_argument("--chunk-ms", type=int, default=100)
    args = parser.parse_args()

    if not os.getenv("OPENAI_API_KEY"):
        raise SystemExit("OPENAI_API_KEY is required")

    with wave.open(str(args.wav), "rb") as audio:
        if (audio.getframerate(), audio.getnchannels(), audio.getsampwidth()) != (24000, 1, 2):
            raise SystemExit("WAV must be 24 kHz mono PCM16")
        pcm = audio.readframes(audio.getnframes())
        rate = audio.getframerate()

    started = time.monotonic()
    arrivals: list[tuple[float, str]] = []

    def on_delta(text: str) -> None:
        elapsed = time.monotonic() - started
        arrivals.append((elapsed, text))
        print(f"DELTA t={elapsed:.3f}s text={text!r}", flush=True)

    stream = RealtimeTranscriptionStream(OpenAI(), on_delta)
    chunk_bytes = rate * 2 * args.chunk_ms // 1000
    for offset in range(0, len(pcm), chunk_bytes):
        stream.append(pcm[offset : offset + chunk_bytes])
        time.sleep(args.chunk_ms / 1000)

    audio_sent_at = time.monotonic() - started
    deltas_before_finalize = len(arrivals)
    print(
        f"AUDIO_SENT t={audio_sent_at:.3f}s deltas_before_finalize={deltas_before_finalize}",
        flush=True,
    )
    final = stream.finalize()

    if not arrivals or arrivals[0][0] >= audio_sent_at:
        raise SystemExit("FAIL: no transcript delta arrived before the audio upload completed")

    print(
        "PASS "
        f"first_delta={arrivals[0][0]:.3f}s "
        f"lead={audio_sent_at - arrivals[0][0]:.3f}s "
        f"delta_count={len(arrivals)} final={final!r}"
    )


if __name__ == "__main__":
    main()
