"""The transcription and tidy models, from the one shared list.

The list lives in packages/protocol/models.json, which the desktop app also
reads. The packaged worker carries a copy beside this file (the PyInstaller
build adds it); a worker run from the repository reads the shared file.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

_FILE_NAME = "models.json"


def shared_models_path() -> Path:
    bundled = Path(__file__).with_name(_FILE_NAME)
    if bundled.is_file():
        return bundled
    return Path(__file__).resolve().parents[4] / "packages" / "protocol" / _FILE_NAME


def _load() -> dict[str, Any]:
    with shared_models_path().open(encoding="utf-8") as file:
        return json.load(file)


_SHARED = _load()

DEFAULT_TRANSCRIBE_MODEL: str = _SHARED["default_transcription_model"]
LIVE_TRANSCRIBE_MODEL: str = _SHARED["live_transcription_model"]
TRANSCRIPTION_MODELS: tuple[str, ...] = tuple(
    entry["id"] for entry in _SHARED["transcription_models"]
)
# Settings saved before the gpt-transcribe family keep working after upgrade.
LEGACY_MODEL_ALIASES: dict[str, str] = dict(_SHARED["legacy_transcription_models"])
# The desktop settings text names this model; both read it from the shared list.
DEFAULT_POLISH_MODEL: str = _SHARED["refine_model"]
