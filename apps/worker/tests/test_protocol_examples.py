from __future__ import annotations

import io
import json
import unittest
import uuid
from pathlib import Path
from typing import Any
from unittest.mock import patch

from openwhisper_worker.protocol import SessionEvent, WorkerRequest
from openwhisper_worker.server import WorkerServer, serve

EXAMPLES = Path(__file__).resolve().parents[3] / "packages" / "protocol" / "examples"


def example(name: str) -> dict[str, Any]:
    return json.loads((EXAMPLES / f"{name}.json").read_text(encoding="utf-8"))


class FileOpenAI:
    """Stands in for the OpenAI client a start_session request names."""

    is_live = False

    def __init__(self, model: str) -> None:
        self.model = model


class ProtocolExampleTests(unittest.TestCase):
    """The worker answers the shared example requests with the shared replies."""

    def setUp(self) -> None:
        self.server = WorkerServer(
            emit=lambda _event: None,
            openai_factory=lambda _key, model: FileOpenAI(model),
        )

    def answer(self, name: str) -> tuple[dict[str, Any], dict[str, Any]]:
        exchange = example(name)
        request = WorkerRequest.model_validate(exchange["request"])
        reply = json.loads(self.server.handle(request).model_dump_json())
        return reply, exchange["reply"]

    def test_a_started_then_stopped_session_answers_as_the_examples_show(self) -> None:
        session_id = example("start_session")["reply"]["result"]["session_id"]
        with patch(
            "openwhisper_worker.server.uuid.uuid4", return_value=uuid.UUID(session_id)
        ), patch("sys.stderr", io.StringIO()):
            for name in ("start_session", "stop_session", "unknown_session"):
                with self.subTest(example=name):
                    reply, expected = self.answer(name)
                    self.assertEqual(reply, expected)

    def test_list_models_answers_with_the_shared_list(self) -> None:
        reply, expected = self.answer("list_models")

        self.assertEqual(reply, expected)

    def test_the_example_session_event_is_one_the_worker_writes(self) -> None:
        line = example("session_event")["event"]
        event = SessionEvent.model_validate(line)

        self.assertEqual(json.loads(event.model_dump_json()), line)

    def test_each_example_is_one_line_on_the_wire(self) -> None:
        # The stdio loop reads one request per line and writes one reply per line.
        requests = io.StringIO(json.dumps(example("list_models")["request"]) + "\n")
        replies = io.StringIO()
        serve(requests, replies)

        lines = replies.getvalue().splitlines()
        self.assertEqual(len(lines), 1)
        self.assertEqual(json.loads(lines[0]), example("list_models")["reply"])


if __name__ == "__main__":
    unittest.main()
