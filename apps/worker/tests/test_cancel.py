from __future__ import annotations

import base64
import io
import json
import os
import queue
import threading
import unittest
from unittest.mock import patch

from openwhisper_worker.protocol import WorkerRequest
from openwhisper_worker.server import WorkerServer, serve
from test_realtime_server import EventLog, pcm_tone

_WAIT_SECONDS = 5


class SlowFileOpenAI:
    """Transcribes in parts and waits inside each part until released."""

    model = "gpt-transcribe"
    is_live = False

    def __init__(self, parts: int = 3) -> None:
        self.parts = parts
        self.uploaded = 0
        self.polished: list[str] = []
        self.part_started = threading.Event()
        self.release = threading.Event()
        self.on_transcribed = lambda: None

    def transcribe_file(self, _path, mime_type: str, prompt: str = "", on_progress=None) -> str:
        for part in range(1, self.parts + 1):
            if on_progress:
                on_progress(part, self.parts)
            self.uploaded += 1
            self.part_started.set()
            self.release.wait(_WAIT_SECONDS)
        self.on_transcribed()
        return "Um, please send the report by Friday."

    def polish(self, text: str, _prompt: str) -> str:
        self.polished.append(text)
        return text


def line(request_id: str, method: str, **params) -> str:
    return json.dumps({"id": request_id, "method": method, "params": params}) + "\n"


class RunningWorker:
    """Runs the real stdio request loop on pipes, as the desktop app does."""

    def __init__(self, openai) -> None:
        self.openai = openai
        read_in, self._write_in = os.pipe()
        read_out, write_out = os.pipe()
        self._requests = os.fdopen(read_in, "r", encoding="utf-8")
        self._input = os.fdopen(self._write_in, "w", encoding="utf-8")
        self._responses = os.fdopen(write_out, "w", encoding="utf-8")
        self._output = os.fdopen(read_out, "r", encoding="utf-8")
        self.lines: queue.Queue[dict] = queue.Queue()

        def make_server(emit) -> WorkerServer:
            server = WorkerServer(emit=emit)
            server._openai = openai
            return server

        self._server_thread = threading.Thread(
            target=self._serve, args=(make_server,), daemon=True
        )
        self._reader_thread = threading.Thread(target=self._read, daemon=True)
        self._server_thread.start()
        self._reader_thread.start()

    def _serve(self, make_server) -> None:
        try:
            serve(self._requests, self._responses, make_server)
        finally:
            self._responses.close()

    def _read(self) -> None:
        for raw in self._output:
            self.lines.put(json.loads(raw))

    def send(self, payload: str) -> None:
        self._input.write(payload)
        self._input.flush()

    def next_line(self) -> dict:
        return self.lines.get(timeout=_WAIT_SECONDS)

    def response(self, request_id: str, seen: list[dict]) -> dict:
        while True:
            message = self.next_line()
            seen.append(message)
            if message.get("id") == request_id:
                return message

    def close(self) -> None:
        self._input.close()
        self._server_thread.join(_WAIT_SECONDS)


class CancelTests(unittest.TestCase):
    def setUp(self) -> None:
        stderr = patch("sys.stderr", io.StringIO())
        stderr.start()
        self.addCleanup(stderr.stop)

    def test_cancel_answers_while_a_long_transcription_is_still_running(self) -> None:
        openai = SlowFileOpenAI(parts=3)
        worker = RunningWorker(openai)
        self.addCleanup(worker.close)
        self.addCleanup(openai.release.set)
        seen: list[dict] = []

        worker.send(line("start", "start_session", refine_enabled=True))
        session_id = worker.response("start", seen)["result"]["session_id"]
        audio = base64.b64encode(pcm_tone(1.0)).decode("ascii")
        worker.send(line("append", "append_audio_chunk", session_id=session_id, chunk_base64=audio))
        worker.response("append", seen)

        worker.send(
            line(
                "finalize",
                "finalize_session_audio",
                session_id=session_id,
                mime_type="audio/pcm;rate=24000",
            )
        )
        self.assertTrue(openai.part_started.wait(_WAIT_SECONDS))
        # Progress arrives while the first part is still being transcribed.
        progress = worker.next_line()
        self.assertEqual(
            progress,
            {
                "session_id": session_id,
                "event": {"type": "partial", "text": "Transcribing part 1 of 3..."},
                "done": False,
            },
        )

        worker.send(line("stop", "stop_session", session_id=session_id))
        stopped = worker.response("stop", seen)
        self.assertTrue(stopped["ok"])
        self.assertNotIn("finalize", [message.get("id") for message in seen])

        openai.release.set()
        finalized = worker.response("finalize", seen)
        self.assertEqual(finalized["result"], {"final_text": "", "cancelled": True})
        # The job stopped before the next audio part and never tidied or
        # delivered a transcript.
        self.assertEqual(openai.uploaded, 1)
        self.assertEqual(openai.polished, [])
        events = [m["event"]["type"] for m in seen if m.get("session_id") == session_id]
        self.assertNotIn("final", events)

    def test_cancel_after_transcription_skips_tidying(self) -> None:
        openai = SlowFileOpenAI(parts=1)
        openai.release.set()
        log = EventLog()
        server = WorkerServer(emit=log)
        server._openai = openai
        session_id = server.handle(
            WorkerRequest(id="start", method="start_session", params={"refine_enabled": True})
        ).result["session_id"]
        server.handle(
            WorkerRequest(
                id="append",
                method="append_audio_chunk",
                params={
                    "session_id": session_id,
                    "chunk_base64": base64.b64encode(pcm_tone(1.0)).decode("ascii"),
                },
            )
        )
        stop = WorkerRequest(id="stop", method="stop_session", params={"session_id": session_id})
        openai.on_transcribed = lambda: self.assertTrue(server.handle(stop).ok)

        finalized = server.handle(
            WorkerRequest(
                id="finalize",
                method="finalize_session_audio",
                params={"session_id": session_id, "mime_type": "audio/pcm;rate=24000"},
            )
        )

        self.assertEqual(finalized.result, {"final_text": "", "cancelled": True})
        self.assertEqual(openai.polished, [])
        self.assertNotIn("final", [event["type"] for event in log.events(session_id)])
        self.assertEqual(server._sessions, {})

    def test_a_failed_session_is_forgotten(self) -> None:
        log = EventLog()
        server = WorkerServer(emit=log)
        server._openai = SlowFileOpenAI()
        session_id = server.handle(
            WorkerRequest(id="start", method="start_session")
        ).result["session_id"]
        server.handle(
            WorkerRequest(
                id="finalize",
                method="finalize_session_audio",
                params={"session_id": session_id, "mime_type": "audio/pcm;rate=24000"},
            )
        )

        self.assertEqual(log.events(session_id), [{"type": "error", "text": "No audio found"}])
        self.assertTrue(log.done(session_id))
        self.assertEqual(server._sessions, {})


if __name__ == "__main__":
    unittest.main()
