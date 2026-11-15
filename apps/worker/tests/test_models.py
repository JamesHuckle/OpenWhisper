from __future__ import annotations

import json
import os
import unittest
from pathlib import Path
from unittest.mock import patch

from openwhisper_worker import models
from openwhisper_worker.openai_client import OpenWhisperOpenAI

SHARED_FILE = Path(__file__).resolve().parents[3] / "packages" / "protocol" / "models.json"


def default_client() -> OpenWhisperOpenAI:
    """A client with no model chosen in the environment."""
    with patch.dict(os.environ):
        os.environ.pop("OPENWHISPER_MODEL", None)
        os.environ.pop("OPENWHISPER_POLISH_MODEL", None)
        return OpenWhisperOpenAI()


class SharedModelListTests(unittest.TestCase):
    """The worker reads the one model list the desktop app also reads."""

    def test_a_worker_run_from_the_repository_reads_the_shared_file(self) -> None:
        self.assertEqual(models.shared_models_path(), SHARED_FILE)

    def test_the_worker_offers_exactly_the_shared_models(self) -> None:
        shared = json.loads(SHARED_FILE.read_text(encoding="utf-8"))

        self.assertEqual(
            OpenWhisperOpenAI().available_models(),
            [entry["id"] for entry in shared["transcription_models"]],
        )
        self.assertEqual(default_client().model, shared["default_transcription_model"])
        for legacy, current in shared["legacy_transcription_models"].items():
            with patch.dict(os.environ, {"OPENWHISPER_MODEL": legacy}):
                self.assertEqual(OpenWhisperOpenAI().model, current)

    def test_the_tidy_pass_uses_the_shared_refine_model(self) -> None:
        shared = json.loads(SHARED_FILE.read_text(encoding="utf-8"))

        self.assertEqual(default_client().health()["polish_model"], shared["refine_model"])


if __name__ == "__main__":
    unittest.main()
