from __future__ import annotations

import base64
import unittest

from openwhisper_worker.protocol import WorkerRequest
from openwhisper_worker.server import WorkerServer


class FakeRealtimeStream:
    def __init__(self, on_delta) -> None:
        self.on_delta = on_delta
        self.chunks: list[bytes] = []
        self.closed = False

    def append(self, chunk: bytes) -> None:
        self.chunks.append(chunk)
        self.on_delta("hello")

    def finalize(self) -> str:
        return "hello world"

    def close(self) -> None:
        self.closed = True


class FakeRealtimeOpenAI:
    model = "gpt-realtime-transcribe"

    def __init__(self) -> None:
        self.stream: FakeRealtimeStream | None = None

    def start_realtime_transcription(self, on_delta) -> FakeRealtimeStream:
        self.stream = FakeRealtimeStream(on_delta)
        return self.stream

    def polish(self, text: str, _prompt: str) -> str:
        return text


def request(method: str, **params) -> WorkerRequest:
    return WorkerRequest(id=method, method=method, params=params)


class RealtimeWorkerServerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.server = WorkerServer()
        self.fake_openai = FakeRealtimeOpenAI()
        self.server._openai = self.fake_openai

    def test_audio_is_forwarded_and_delta_is_pollable_before_finalize(self) -> None:
        started = self.server.handle(
            request("start_session", refine_enabled=False)
        )
        self.assertTrue(started.ok)
        session_id = started.result["session_id"]

        # Consume the initial Listening status, then stream one PCM chunk.
        self.server.handle(request("poll_session_events", session_id=session_id))
        audio = b"\x01\x02\x03\x04"
        appended = self.server.handle(
            request(
                "append_audio_chunk",
                session_id=session_id,
                chunk_base64=base64.b64encode(audio).decode("ascii"),
            )
        )
        self.assertTrue(appended.ok)
        self.assertEqual(self.fake_openai.stream.chunks, [audio])

        polled = self.server.handle(request("poll_session_events", session_id=session_id))
        self.assertEqual(polled.result["events"], [{"type": "partial", "text": "hello"}])
        self.assertFalse(polled.result["done"])

        finalized = self.server.handle(
            request("finalize_session_audio", session_id=session_id, mime_type="audio/pcm;rate=24000")
        )
        self.assertEqual(finalized.result["final_text"], "hello world")
        final_event = self.server.handle(
            request("poll_session_events", session_id=session_id)
        )
        self.assertEqual(
            final_event.result["events"], [{"type": "final", "text": "hello world"}]
        )
        self.assertTrue(final_event.result["done"])


if __name__ == "__main__":
    unittest.main()
