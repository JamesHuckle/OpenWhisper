"""The transcription and tidy models, from the one shared list.

The list lives in packages/protocol/models.json, which the desktop app also
reads. The packaged worker carries a copy beside this file (the PyInstaller
build adds it); a worker run from the repository reads the shared file.
"""

from __future__ import annotations

from pathlib import Path

from openwhisper_worker.shared_files import load_shared_json, shared_file_path

_FILE_NAME = "models.json"


def shared_models_path() -> Path:
    return shared_file_path(_FILE_NAME)


_SHARED = load_shared_json(_FILE_NAME)

DEFAULT_TRANSCRIBE_MODEL: str = _SHARED["default_transcription_model"]
LIVE_TRANSCRIBE_MODEL: str = _SHARED["live_transcription_model"]
TRANSCRIPTION_MODELS: tuple[str, ...] = tuple(
    entry["id"] for entry in _SHARED["transcription_models"]
)
# Settings saved before the gpt-transcribe family keep working after upgrade.
LEGACY_MODEL_ALIASES: dict[str, str] = dict(_SHARED["legacy_transcription_models"])
# The desktop settings text names this model; both read it from the shared list.
DEFAULT_POLISH_MODEL: str = _SHARED["refine_model"]
