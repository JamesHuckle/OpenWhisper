"""Write the example WAV files in this folder.

Run from the repository root: python packages/protocol/audio-examples/make_examples.py
The files are committed; rerun this only to change them, and keep cases.json in step.
"""

from __future__ import annotations

import math
import struct
from pathlib import Path

RATE = 16_000
HERE = Path(__file__).resolve().parent


def tone(seconds: float, peak: int = 8_000) -> list[int]:
    count = round(seconds * RATE)
    return [round(peak * math.sin(2 * math.pi * 440 * i / RATE)) for i in range(count)]


def write(name: str, samples: list[int], extra_chunk: bool = False) -> None:
    pcm = struct.pack(f"<{len(samples)}h", *samples)
    fmt = struct.pack("<4sIHHIIHH", b"fmt ", 16, 1, 1, RATE, RATE * 2, 2, 16)
    # A LIST chunk before the audio, as some recorders write.
    info = struct.pack("<4sI4s", b"LIST", 4, b"INFO") if extra_chunk else b""
    data = struct.pack("<4sI", b"data", len(pcm)) + pcm
    body = b"WAVE" + fmt + info + data
    (HERE / name).write_bytes(struct.pack("<4sI", b"RIFF", len(body)) + body)


write("empty.wav", [])
write("tap.wav", tone(0.2))
write("shortest-speech.wav", tone(0.3))
write("muted.wav", [99 if i % 2 else -99 for i in range(RATE)])
quiet = [0] * RATE
quiet[RATE // 2] = -100
write("quiet-speech.wav", quiet)
write("late-speech.wav", [0] * (RATE * 3 // 2) + tone(0.5), extra_chunk=True)
