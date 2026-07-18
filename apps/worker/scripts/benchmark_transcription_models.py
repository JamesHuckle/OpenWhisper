from __future__ import annotations

import argparse
import re
import statistics
import time
from pathlib import Path

from openai import OpenAI


MODELS = ("gpt-4o-mini-transcribe", "gpt-4o-transcribe")


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


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Compare supported OpenWhisper transcription models on the same audio."
    )
    parser.add_argument("audio", type=Path)
    parser.add_argument("--reference", help="Expected transcript used to calculate word error rate")
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--prompt", default="")
    args = parser.parse_args()

    if args.runs < 1:
        parser.error("--runs must be at least 1")
    if not args.audio.is_file():
        parser.error(f"Audio file does not exist: {args.audio}")

    client = OpenAI()
    for model in MODELS:
        durations: list[float] = []
        transcripts: list[str] = []
        for _ in range(args.runs):
            with args.audio.open("rb") as audio:
                started = time.perf_counter()
                request = {"model": model, "file": audio}
                if args.prompt:
                    request["prompt"] = args.prompt
                response = client.audio.transcriptions.create(**request)
                durations.append(time.perf_counter() - started)
                transcripts.append(response.text.strip())

        median_seconds = statistics.median(durations)
        wer = word_error_rate(args.reference, transcripts[-1]) if args.reference else None
        accuracy = f"{(1 - wer) * 100:.1f}%" if wer is not None else "n/a"
        print(
            f"{model}: median={median_seconds:.3f}s accuracy={accuracy} "
            f"runs={','.join(f'{duration:.3f}' for duration in durations)}"
        )
        print(f"  transcript: {transcripts[-1]}")


if __name__ == "__main__":
    main()
