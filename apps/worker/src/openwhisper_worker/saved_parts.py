"""The finished parts of a long transcription, saved beside its recording.

A long recording is sent in parts. Each part's text is saved as soon as it
arrives, so a retry after a failure uploads (and pays for) only the parts that
did not finish. The file is tagged with a key made from the audio's size and
modified time, the model, the prompt and the chunking version: when any of
them changes, the saved parts are ignored and the transcription starts fresh.

The desktop names the file (`parts.json` in the recording's folder) and
deletes it when the transcription completes or the recording is pruned.
"""

from __future__ import annotations

import hashlib
import json
import os
import sys
from dataclasses import asdict, dataclass
from pathlib import Path

# Raise this when the way a recording is split into parts, or the prompt each
# part is sent with, changes: older saved parts then no longer match.
CHUNKING_VERSION = 1


@dataclass(frozen=True)
class SavedPart:
    # The part's byte range in the recording's audio data.
    start: int
    end: int
    # The prompt the part was sent with.
    prompt: str
    # The text the service returned, which may be empty for a silent part.
    text: str


def parts_key(audio_path: Path, model: str, prompt: str) -> str:
    """The key saved parts must carry to be used for this audio and these settings."""
    stat = audio_path.stat()
    material = json.dumps(
        {
            "version": CHUNKING_VERSION,
            "audio_bytes": stat.st_size,
            "audio_modified_ns": stat.st_mtime_ns,
            "model": model,
            "prompt": prompt,
        },
        sort_keys=True,
    )
    return hashlib.sha256(material.encode("utf-8")).hexdigest()


class SavedParts:
    """The parts saved for one key. Without a path nothing is read or saved."""

    def __init__(self, path: Path | None, key: str) -> None:
        self._path = path
        self._key = key
        self._parts: list[SavedPart] = self._load() if path else []

    def get(self, index: int, start: int, end: int) -> SavedPart | None:
        """The saved part `index`, when it covers exactly these bytes."""
        if index >= len(self._parts):
            return None
        part = self._parts[index]
        return part if (part.start, part.end) == (start, end) else None

    def add(self, index: int, part: SavedPart) -> None:
        """Keeps a finished part and saves every part so far, atomically.

        A part that cannot be saved is only logged: the transcription goes on,
        and a retry sends that part again.
        """
        del self._parts[index:]
        self._parts.append(part)
        if self._path is None:
            return
        try:
            _write_json_atomic(
                self._path,
                {"key": self._key, "parts": [asdict(saved) for saved in self._parts]},
            )
        except OSError as exc:
            sys.stderr.write(f"[parts] save_failed error={exc.__class__.__name__}\n")
            sys.stderr.flush()

    def _load(self) -> list[SavedPart]:
        assert self._path is not None
        try:
            saved = json.loads(self._path.read_text(encoding="utf-8"))
            if saved.get("key") != self._key:
                return []
            parts = [
                SavedPart(
                    start=int(part["start"]),
                    end=int(part["end"]),
                    prompt=str(part["prompt"]),
                    text=str(part["text"]),
                )
                for part in saved["parts"]
            ]
        except FileNotFoundError:
            return []
        except (OSError, ValueError, TypeError, KeyError, AttributeError) as exc:
            # A damaged file is the same as none: start fresh.
            sys.stderr.write(f"[parts] unreadable error={exc.__class__.__name__}\n")
            sys.stderr.flush()
            return []
        if parts:
            sys.stderr.write(f"[parts] resuming saved_parts={len(parts)}\n")
            sys.stderr.flush()
        return parts


def _write_json_atomic(path: Path, value: object) -> None:
    """Writes `value` beside `path` and moves it into place, so a crash while
    saving leaves the previous file, never a half-written one."""
    temporary = path.with_name(f".{path.name}.tmp")
    with temporary.open("w", encoding="utf-8") as file:
        json.dump(value, file)
        file.flush()
        os.fsync(file.fileno())
    os.replace(temporary, path)
