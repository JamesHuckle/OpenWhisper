from __future__ import annotations

import base64
import io
import math
import struct
import unittest
from unittest.mock import patch

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
        self.closed = True
        return "hello world"

    def close(self) -> None:
        self.closed = True


class FakeRealtimeOpenAI:
    model = "gpt-live-transcribe"
    is_live = True

    def __init__(self) -> None:
        self.stream: FakeRealtimeStream | None = None
        self.streams: list[FakeRealtimeStream] = []

    def start_realtime_transcription(self, on_delta) -> FakeRealtimeStream:
        self.stream = FakeRealtimeStream(on_delta)
        self.streams.append(self.stream)
        return self.stream

    def polish(self, text: str, _prompt: str) -> str:
        return text


class FakeFileOpenAI:
    model = "gpt-transcribe"
    is_live = False

    def __init__(
        self,
        transcript: str,
        transcribe_error: Exception | None = None,
        polish_error: Exception | None = None,
    ) -> None:
        self.transcript = transcript
        self.transcribe_error = transcribe_error
        self.polish_error = polish_error
        self.transcribed = 0
        self.polished: list[str] = []

    def transcribe_file(self, _path, mime_type: str, prompt: str = "", on_progress=None) -> str:
        self.transcribed += 1
        if self.transcribe_error:
            raise self.transcribe_error
        return self.transcript

    def polish(self, text: str, _prompt: str) -> str:
        self.polished.append(text)
        if self.polish_error:
            raise self.polish_error
        return f"tidied: {text}"


def request(method: str, **params) -> WorkerRequest:
    return WorkerRequest(id=method, method=method, params=params)


def pcm_tone(seconds: float, amplitude: int = 6000, rate: int = 24_000) -> bytes:
    frames = int(seconds * rate)
    return b"".join(
        struct.pack("<h", int(amplitude * math.sin(2 * math.pi * 220 * i / rate)))
        for i in range(frames)
    )


def dictate(
    server: WorkerServer,
    audio: bytes,
    prompt: str = "",
    refine_prompt: str = "",
    refine_enabled: bool | None = None,
) -> list[dict]:
    if refine_enabled is None:
        refine_enabled = bool(refine_prompt)
    session_id = server.handle(
        request(
            "start_session",
            prompt=prompt,
            refine_enabled=refine_enabled,
            refine_prompt=refine_prompt,
        )
    ).result["session_id"]
    server.handle(
        request(
            "append_audio_chunk",
            session_id=session_id,
            chunk_base64=base64.b64encode(audio).decode("ascii"),
        )
    )
    server.handle(
        request("finalize_session_audio", session_id=session_id, mime_type="audio/pcm;rate=24000")
    )
    return server.handle(request("poll_session_events", session_id=session_id)).result["events"]


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
        audio = pcm_tone(0.5)
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

    def test_long_realtime_audio_rotates_and_stitches_sessions(self) -> None:
        started = self.server.handle(request("start_session", refine_enabled=False))
        session_id = started.result["session_id"]
        with patch("openwhisper_worker.server._REALTIME_ROTATE_BYTES", 4):
            self.server.handle(
                request(
                    "append_audio_chunk",
                    session_id=session_id,
                    chunk_base64=base64.b64encode(b"\x01\x02\x03\x04").decode("ascii"),
                )
            )
            self.server.handle(
                request(
                    "append_audio_chunk",
                    session_id=session_id,
                    chunk_base64=base64.b64encode(b"\x05\x06").decode("ascii"),
                )
            )

        self.assertEqual(len(self.fake_openai.streams), 2)
        self.assertTrue(self.fake_openai.streams[0].closed)
        finalized = self.server.handle(
            request(
                "finalize_session_audio",
                session_id=session_id,
                mime_type="audio/pcm;rate=24000",
            )
        )
        self.assertEqual(finalized.result["final_text"], "hello world hello world")


class NoAudioWorkerServerTests(unittest.TestCase):
    PROMPT = "Glossary:\nZoon\nZoon Robotics\nEuphoric"

    def server_returning(self, transcript: str) -> tuple[WorkerServer, FakeFileOpenAI]:
        server = WorkerServer()
        server._openai = FakeFileOpenAI(transcript)
        return server, server._openai

    def test_quick_tap_and_silence_report_no_audio_without_transcribing(self) -> None:
        for audio in (pcm_tone(0.1), bytes(24_000 * 2)):
            server, openai = self.server_returning("Glossary: Zoon Zoon Robotics Euphoric")
            events = dictate(server, audio, self.PROMPT)
            self.assertEqual(events[-1], {"type": "error", "text": "No audio found"})
            self.assertEqual(openai.transcribed, 0)

    def test_dictionary_echo_is_not_pasted_but_real_speech_is(self) -> None:
        server, _ = self.server_returning("Glossary: Zoon Zoon Robotics Euphoric")
        events = dictate(server, pcm_tone(1.0), self.PROMPT)
        self.assertEqual(events[-1], {"type": "error", "text": "No audio found"})

        server, _ = self.server_returning("Zoon Robotics is hiring.")
        events = dictate(server, pcm_tone(1.0), self.PROMPT)
        self.assertEqual(events[-1], {"type": "final", "text": "Zoon Robotics is hiring."})


class RefineWorkerServerTests(unittest.TestCase):
    def dictate_with_refine(self, transcript: str, refine_prompt: str = "") -> tuple:
        server = WorkerServer()
        openai = FakeFileOpenAI(transcript)
        server._openai = openai
        stderr = io.StringIO()
        with patch("sys.stderr", stderr):
            events = dictate(server, pcm_tone(1.0), refine_prompt=refine_prompt, refine_enabled=True)
        return events, openai, stderr.getvalue()

    def test_short_clean_transcript_is_pasted_without_a_tidy_call(self) -> None:
        events, openai, log = self.dictate_with_refine("Send me the report by Friday.")

        self.assertEqual(openai.polished, [])
        self.assertNotIn({"type": "partial", "text": "Refining transcript..."}, events)
        self.assertEqual(events[-1], {"type": "final", "text": "Send me the report by Friday."})
        self.assertIn("refine skipped", log)
        self.assertIn("reason=short_clean", log)

    def test_transcripts_that_need_tidying_still_get_the_tidy_call(self) -> None:
        for transcript in (
            "Um, send me the report by Friday.",
            "Send me the report by Friday. Thanks a lot.",
            "Please send me the full quarterly sales report and the budget by Friday morning.",
            "send me the report by friday",
        ):
            with self.subTest(transcript=transcript):
                events, openai, _ = self.dictate_with_refine(transcript)
                self.assertEqual(openai.polished, [transcript])
                self.assertEqual(events[-1], {"type": "final", "text": f"tidied: {transcript}"})

    def test_custom_refine_prompt_always_gets_the_tidy_call(self) -> None:
        events, openai, _ = self.dictate_with_refine(
            "Send me the report by Friday.", refine_prompt="Translate into French."
        )
        self.assertEqual(openai.polished, ["Send me the report by Friday."])
        self.assertEqual(
            events[-1], {"type": "final", "text": "tidied: Send me the report by Friday."}
        )


class PrivateTextWorkerServerTests(unittest.TestCase):
    DICTATED = "My bank PIN is four four seven one, please remind me at noon."
    VOCABULARY = "Glossary:\nGwendolyn\nZoon Robotics"
    REFINE_PROMPT = "Write my mother's maiden name, Gwendolyn, in capitals."

    def test_failed_refinement_keeps_transcript_out_of_log_and_notice(self) -> None:
        server = WorkerServer()
        server._openai = FakeFileOpenAI(
            self.DICTATED,
            polish_error=RuntimeError(f"Could not tidy: {self.DICTATED} {self.REFINE_PROMPT}"),
        )
        stderr = io.StringIO()

        with patch("sys.stderr", stderr):
            events = dictate(server, pcm_tone(1.0), self.VOCABULARY, self.REFINE_PROMPT)

        warning = next(event for event in events if event["type"] == "warning")
        self.assertEqual(
            warning["text"], "Refinement failed; using the raw transcript (RuntimeError)"
        )
        self.assertEqual(events[-1], {"type": "final", "text": self.DICTATED})
        log = stderr.getvalue()
        self.assertIn("refine failed", log)
        for private in ("bank PIN", "Gwendolyn", "Glossary"):
            self.assertNotIn(private, log)
            self.assertNotIn(private, warning["text"])

    def test_failed_transcription_notice_leaves_out_the_vocabulary_prompt(self) -> None:
        server = WorkerServer()
        server._openai = FakeFileOpenAI(
            "",
            transcribe_error=RuntimeError(f"Prompt rejected: {self.VOCABULARY}"),
        )
        stderr = io.StringIO()

        with patch("sys.stderr", stderr):
            events = dictate(server, pcm_tone(1.0), self.VOCABULARY)

        self.assertEqual(events[-1], {"type": "error", "text": "Prompt rejected: [redacted]"})
        self.assertNotIn("Gwendolyn", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
