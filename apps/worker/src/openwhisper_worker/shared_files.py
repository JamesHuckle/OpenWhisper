"""Find a file from packages/protocol, the files every app reads.

The packaged worker carries copies beside this module (the PyInstaller build
adds them); a worker run from the repository reads the shared files.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any


def shared_file_path(name: str) -> Path:
    bundled = Path(__file__).with_name(name)
    if bundled.is_file():
        return bundled
    return Path(__file__).resolve().parents[4] / "packages" / "protocol" / name


def load_shared_json(name: str) -> dict[str, Any]:
    with shared_file_path(name).open(encoding="utf-8") as file:
        return json.load(file)
