from __future__ import annotations

import base64
import io
import json
import os
import queue
import tempfile
import threading
import unittest
from unittest.mock import patch

from openwhisper_worker.protocol import WorkerRequest
from openwhisper_worker.server import WorkerServer, serve
from test_realtime_server import EventLog, FakeRealtimeOpenAI, pcm_tone, write_wav

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

    def transcribe_file(
        self,
        _path,
        mime_type: str,
        prompt: str = "",
        on_progress=None,
        parts_path=None,
        upload_parts=(),
        upload_mime_type: str = "",
    ) -> str:
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

    def saved_recording(self) -> str:
        """A second of speech saved as the desktop saves a recording."""
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        path = os.path.join(folder.name, "audio.wav")
        write_wav(path, pcm_tone(1.0))
        return path

    def test_cancel_answers_while_a_long_transcription_is_still_running(self) -> None:
        openai = SlowFileOpenAI(parts=3)
        worker = RunningWorker(openai)
        self.addCleanup(worker.close)
        self.addCleanup(openai.release.set)
        seen: list[dict] = []

        worker.send(line("start", "start_session", refine_enabled=True))
        session_id = worker.response("start", seen)["result"]["session_id"]

        worker.send(
            line(
                "finalize",
                "finalize_session_audio",
                session_id=session_id,
                mime_type="audio/wav",
                audio_path=self.saved_recording(),
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
        stop = WorkerRequest(id="stop", method="stop_session", params={"session_id": session_id})
        openai.on_transcribed = lambda: self.assertTrue(server.handle(stop).ok)

        finalized = server.handle(
            WorkerRequest(
                id="finalize",
                method="finalize_session_audio",
                params={
                    "session_id": session_id,
                    "mime_type": "audio/wav",
                    "audio_path": self.saved_recording(),
                },
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


    def test_a_chunk_that_arrives_after_stop_is_refused(self) -> None:
        openai = FakeRealtimeOpenAI()
        server = WorkerServer(emit=EventLog())
        server._openai = openai
        session_id = server.handle(
            WorkerRequest(id="start", method="start_session")
        ).result["session_id"]
        session = server._sessions[session_id]
        stop = WorkerRequest(id="stop", method="stop_session", params={"session_id": session_id})
        self.assertTrue(server.handle(stop).ok)

        # The append looked the session up before stop_session removed it.
        with patch.object(server, "_session", return_value=session):
            appended = server.handle(
                WorkerRequest(
                    id="append",
                    method="append_audio_chunk",
                    params={
                        "session_id": session_id,
                        "chunk_base64": base64.b64encode(pcm_tone(0.1)).decode("ascii"),
                    },
                )
            )

        self.assertFalse(appended.ok)
        self.assertEqual(session.audio_bytes, 0)
        self.assertEqual(openai.stream.chunks, [])

    def test_stop_during_live_rotation_opens_no_new_stream(self) -> None:
        openai = FakeRealtimeOpenAI()
        server = WorkerServer(emit=EventLog())
        server._openai = openai
        session_id = server.handle(
            WorkerRequest(id="start", method="start_session", params={"refine_enabled": False})
        ).result["session_id"]
        stop = WorkerRequest(id="stop", method="stop_session", params={"session_id": session_id})
        first = openai.streams[0]
        finalize = first.finalize

        def finalize_then_stop() -> str:
            # stop_session arrives while the old stream is being finalized.
            self.assertTrue(server.handle(stop).ok)
            return finalize()

        first.finalize = finalize_then_stop
        session = server._sessions[session_id]

        def append(chunk: bytes):
            with patch.object(server, "_session", return_value=session):
                return server.handle(
                    WorkerRequest(
                        id="append",
                        method="append_audio_chunk",
                        params={
                            "session_id": session_id,
                            "chunk_base64": base64.b64encode(chunk).decode("ascii"),
                        },
                    )
                )

        with patch("openwhisper_worker.server._REALTIME_ROTATE_BYTES", 4):
            self.assertTrue(append(b"\x01\x02\x03\x04").ok)
            self.assertFalse(append(b"\x05\x06").ok)

        self.assertTrue(all(stream.closed for stream in openai.streams))
        self.assertEqual(len(openai.streams), 1)

    def test_requests_read_before_input_ends_are_all_answered(self) -> None:
        count = 20
        requests = io.StringIO(
            "".join(line(f"r{n}", "list_models") for n in range(count))
        )
        replies = io.StringIO()
        serve(requests, replies, max_parallel=1)

        ids = sorted(json.loads(reply)["id"] for reply in replies.getvalue().splitlines())
        self.assertEqual(ids, sorted(f"r{n}" for n in range(count)))


if __name__ == "__main__":
    unittest.main()
