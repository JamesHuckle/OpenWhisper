from __future__ import annotations

import argparse
import base64
import re
import statistics
import threading
import time
import wave
from pathlib import Path

from openai import OpenAI


# Batch models upload the finished recording; live models receive PCM while
# the speaker talks, so their latency is measured from the final commit.
BATCH_MODELS = ("gpt-4o-mini-transcribe", "gpt-4o-transcribe", "gpt-transcribe")
LIVE_MODELS = ("gpt-realtime-whisper", "gpt-live-transcribe")
_LIVE_CHUNK_SECONDS = 0.1


def word_error_rate(reference: str, hypothesis: str) -> float:
    def words(text: str) -> list[str]:
        normalized = text.lower().replace("/", " slash ").replace("-", " ").replace("–", " ")
        normalized = re.sub(r"\b([a-z])\.([a-z])\.", r"\1\2", normalized)
        return re.findall(r"[\w]+(?:['’][\w]+)*", normalized)

    expected = words(reference)
    actual = words(hypothesis)
    if not expected:
        return 0.0 if not actual else 1.0

    previous = list(range(len(actual) + 1))
    for row, expected_word in enumerate(expected, start=1):
        current = [row]
        for column, actual_word in enumerate(actual, start=1):
            current.append(
                min(
                    current[column - 1] + 1,
                    previous[column] + 1,
                    previous[column - 1] + (expected_word != actual_word),
                )
            )
        previous = current
    return previous[-1] / len(expected)


def transcribe_batch(client: OpenAI, model: str, audio: Path, prompt: str) -> tuple[float, str]:
    with audio.open("rb") as file:
        request = {"model": model, "file": file, "stream": True}
        if prompt:
            request["prompt"] = prompt
        started = time.perf_counter()
        with client.audio.transcriptions.create(**request) as stream:
            for event in stream:
                if event.type == "transcript.text.done":
                    return time.perf_counter() - started, event.text.strip()
    raise RuntimeError(f"{model} returned no final transcript")


def transcribe_live(client: OpenAI, model: str, audio: Path, prompt: str) -> tuple[float, str]:
    with wave.open(str(audio), "rb") as source:
        if (source.getframerate(), source.getnchannels(), source.getsampwidth()) != (24000, 1, 2):
            raise RuntimeError("Live benchmarks need 24 kHz mono 16-bit WAV audio")
        pcm = source.readframes(source.getnframes())

    transcription: dict[str, object] = {"model": model, "delay": "minimal"}
    if prompt:
        transcription["prompt"] = prompt
    done = threading.Event()
    result: dict[str, str] = {}
    with client.realtime.connect(extra_query={"intent": "transcription"}) as connection:
        connection.session.update(
            session={
                "type": "transcription",
                "audio": {
                    "input": {
                        "format": {"type": "audio/pcm", "rate": 24000},
                        "transcription": transcription,
                        "turn_detection": None,
                    }
                },
            }
        )

        def read_events() -> None:
            for event in connection:
                if event.type == "conversation.item.input_audio_transcription.completed":
                    result["text"] = event.transcript
                    break
                if event.type in {"error", "conversation.item.input_audio_transcription.failed"}:
                    result["error"] = str(getattr(event, "error", event))
                    break
            done.set()

        threading.Thread(target=read_events, daemon=True).start()
        chunk_bytes = int(24000 * 2 * _LIVE_CHUNK_SECONDS)
        for offset in range(0, len(pcm), chunk_bytes):
            connection.input_audio_buffer.append(
                audio=base64.b64encode(pcm[offset : offset + chunk_bytes]).decode("ascii")
            )
            time.sleep(_LIVE_CHUNK_SECONDS)
        started = time.perf_counter()
        connection.input_audio_buffer.commit()
        if not done.wait(120):
            raise RuntimeError(f"{model} timed out")
        elapsed = time.perf_counter() - started
    if "error" in result:
        raise RuntimeError(f"{model} failed: {result['error']}")
    return elapsed, result.get("text", "").strip()


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Compare OpenAI transcription models on the same audio. Batch latency is "
        "upload-to-final-text; live latency is time from the final commit to the final text "
        "after streaming the audio at real-time pace."
    )
    parser.add_argument("audio", type=Path)
    parser.add_argument("--reference", help="Expected transcript used to calculate word error rate")
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--prompt", default="")
    parser.add_argument("--models", nargs="+", default=[*BATCH_MODELS, *LIVE_MODELS])
    args = parser.parse_args()

    if args.runs < 1:
        parser.error("--runs must be at least 1")
    if not args.audio.is_file():
        parser.error(f"Audio file does not exist: {args.audio}")

    client = OpenAI()
    for model in args.models:
        transcribe = transcribe_live if model in LIVE_MODELS else transcribe_batch
        durations: list[float] = []
        transcripts: list[str] = []
        for _ in range(args.runs):
            duration, transcript = transcribe(client, model, args.audio, args.prompt)
            durations.append(duration)
            transcripts.append(transcript)

        median_seconds = statistics.median(durations)
        wer = word_error_rate(args.reference, transcripts[-1]) if args.reference else None
        accuracy = f"{(1 - wer) * 100:.1f}%" if wer is not None else "n/a"
        print(
            f"{model}: median={median_seconds:.3f}s accuracy={accuracy} "
            f"runs={','.join(f'{duration:.3f}' for duration in durations)}",
            flush=True,
        )
        print(f"  transcript: {transcripts[-1]}", flush=True)


if __name__ == "__main__":
    main()
