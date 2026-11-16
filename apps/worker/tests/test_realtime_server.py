from __future__ import annotations

import base64
import io
import math
import os
import struct
import tempfile
import unittest
import wave
from unittest.mock import patch

from openwhisper_worker.audio_rules import NO_AUDIO_MESSAGE
from openwhisper_worker.protocol import SessionEvent, WorkerRequest
from openwhisper_worker.server import WorkerServer


class EventLog:
    """Collects the session events a server pushes, as the desktop would."""

    def __init__(self) -> None:
        self.messages: list[SessionEvent] = []

    def __call__(self, message: SessionEvent) -> None:
        self.messages.append(message)

    def events(self, session_id: str) -> list[dict]:
        return [m.event for m in self.messages if m.session_id == session_id]

    def done(self, session_id: str) -> bool:
        return any(m.done for m in self.messages if m.session_id == session_id)


def new_server(openai) -> tuple[WorkerServer, EventLog]:
    log = EventLog()
    server = WorkerServer(emit=log)
    server._openai = openai
    return server, log


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
        self.files: list[tuple[str, str]] = []
        self.polished: list[str] = []

    def transcribe_file(self, path, mime_type: str, prompt: str = "", on_progress=None) -> str:
        self.transcribed += 1
        self.files.append((str(path), mime_type))
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


def write_wav(path: str, pcm: bytes, rate: int = 24_000) -> None:
    with wave.open(path, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(rate)
        wav.writeframes(pcm)


def finalize_saved(server: WorkerServer, session_id: str, pcm: bytes):
    """Finalize a Standard session as the desktop does: from its saved WAV."""
    with tempfile.TemporaryDirectory() as folder:
        path = os.path.join(folder, "audio.wav")
        write_wav(path, pcm)
        return server.handle(
            request(
                "finalize_session_audio",
                session_id=session_id,
                mime_type="audio/wav",
                audio_path=path,
            )
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
    finalize_saved(server, session_id, audio)
    return server._emit_sink.events(session_id)


class RealtimeWorkerServerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.fake_openai = FakeRealtimeOpenAI()
        self.server, self.log = new_server(self.fake_openai)

    def test_audio_is_forwarded_and_delta_is_pushed_before_finalize(self) -> None:
        started = self.server.handle(
            request("start_session", refine_enabled=False)
        )
        self.assertTrue(started.ok)
        session_id = started.result["session_id"]

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

        self.assertEqual(self.log.events(session_id), [{"type": "partial", "text": "hello"}])
        self.assertFalse(self.log.done(session_id))

        finalized = self.server.handle(
            request("finalize_session_audio", session_id=session_id, mime_type="audio/pcm;rate=24000")
        )
        self.assertEqual(finalized.result["final_text"], "hello world")
        self.assertEqual(
            self.log.events(session_id)[-1], {"type": "final", "text": "hello world"}
        )
        self.assertTrue(self.log.messages[-1].done)
        # A finished session is forgotten, so its transcript leaves memory.
        self.assertEqual(self.server._sessions, {})

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

    def append_and_finalize(
        self, chunks: list[bytes], before_finalize=lambda: None
    ) -> tuple[str, list[dict]]:
        session_id = self.server.handle(
            request("start_session", refine_enabled=False)
        ).result["session_id"]
        for chunk in chunks:
            appended = self.server.handle(
                request(
                    "append_audio_chunk",
                    session_id=session_id,
                    chunk_base64=base64.b64encode(chunk).decode("ascii"),
                )
            )
            self.assertTrue(appended.ok)
        before_finalize()
        with patch("sys.stderr", io.StringIO()):
            self.server.handle(
                request(
                    "finalize_session_audio",
                    session_id=session_id,
                    mime_type="audio/pcm;rate=24000",
                )
            )
        return session_id, self.log.events(session_id)

    def test_silent_or_too_short_live_audio_reports_no_audio_and_keeps_no_copy(self) -> None:
        tenth_of_a_second_of_silence = bytes(2_400 * 2)
        with tempfile.TemporaryDirectory() as temp_dir:
            with patch("tempfile.tempdir", temp_dir):
                for chunks in (
                    [tenth_of_a_second_of_silence] * 10,
                    [pcm_tone(0.1)],
                ):
                    # The desktop saves the recording; the worker writes no
                    # copy, not even while the session is open.
                    _, events = self.append_and_finalize(
                        chunks, lambda: self.assertEqual(os.listdir(temp_dir), [])
                    )
                    self.assertEqual(events[-1], {"type": "error", "text": NO_AUDIO_MESSAGE})
                    self.assertNotIn("final", [event["type"] for event in events])
                    self.assertTrue(self.fake_openai.stream.closed)
                self.assertEqual(os.listdir(temp_dir), [])
        self.assertEqual(self.server._sessions, {})

    def test_one_loud_chunk_among_silent_ones_is_speech(self) -> None:
        silence = bytes(2_400 * 2)
        _, events = self.append_and_finalize([silence] * 5 + [pcm_tone(0.1)] + [silence] * 5)
        self.assertEqual(events[-1], {"type": "final", "text": "hello world"})

    def test_a_standard_session_refuses_audio_chunks(self) -> None:
        server, log = new_server(FakeFileOpenAI("unused"))
        session_id = server.handle(request("start_session")).result["session_id"]

        appended = server.handle(
            request(
                "append_audio_chunk",
                session_id=session_id,
                chunk_base64=base64.b64encode(pcm_tone(0.5)).decode("ascii"),
            )
        )

        self.assertFalse(appended.ok)
        self.assertEqual(appended.error.code, "bad_request")
        self.assertEqual(server._sessions[session_id].audio_bytes, 0)
        self.assertEqual(log.events(session_id), [])


class SessionSettingsWorkerServerTests(unittest.TestCase):
    """Each dictation names its key and model, so saving settings needs no restart."""

    def setUp(self) -> None:
        self.made: list[tuple[str, str, object]] = []

        def make(api_key: str, model: str):
            openai = FakeRealtimeOpenAI() if model == "gpt-live-transcribe" else FakeFileOpenAI(
                f"Said with {api_key}."
            )
            self.made.append((api_key, model, openai))
            return openai

        self.log = EventLog()
        self.server = WorkerServer(emit=self.log, openai_factory=make)
        self.server._openai = FakeFileOpenAI("Said with the environment key.")

    def start(self, api_key: str, model: str) -> str:
        return self.server.handle(
            request(
                "start_session",
                refine_enabled=False,
                openai_api_key=api_key,
                transcription_model=model,
            )
        ).result["session_id"]

    def finalize(self, session_id: str) -> None:
        finalize_saved(self.server, session_id, pcm_tone(1.0))

    def test_dictations_with_the_same_settings_share_one_client(self) -> None:
        first = self.start("key-1", "gpt-transcribe")
        self.finalize(first)
        second = self.start("key-1", "gpt-transcribe")
        self.finalize(second)

        self.assertEqual([(key, model) for key, model, _ in self.made], [("key-1", "gpt-transcribe")])
        self.assertEqual(self.made[0][2].transcribed, 2)
        self.assertEqual(self.log.events(second)[-1], {"type": "final", "text": "Said with key-1."})

    def test_new_settings_apply_to_the_next_dictation_only(self) -> None:
        running = self.start("old-key", "gpt-transcribe")
        changed = self.start("new-key", "gpt-transcribe")
        self.finalize(running)
        self.finalize(changed)

        self.assertEqual(self.log.events(running)[-1], {"type": "final", "text": "Said with old-key."})
        self.assertEqual(self.log.events(changed)[-1], {"type": "final", "text": "Said with new-key."})

    def test_choosing_live_transcription_starts_a_live_session_at_once(self) -> None:
        session_id = self.start("key-1", "gpt-live-transcribe")
        live = self.made[-1][2]

        self.assertIsNotNone(live.stream)
        self.server.handle(
            request(
                "append_audio_chunk",
                session_id=session_id,
                chunk_base64=base64.b64encode(pcm_tone(0.5)).decode("ascii"),
            )
        )
        self.assertEqual(self.log.events(session_id), [{"type": "partial", "text": "hello"}])

    def test_a_dictation_without_settings_uses_the_environment(self) -> None:
        session_id = self.start("", "")
        self.finalize(session_id)

        self.assertEqual(self.made, [])
        self.assertEqual(
            self.log.events(session_id)[-1],
            {"type": "final", "text": "Said with the environment key."},
        )

    def test_an_unknown_model_is_refused_and_the_key_is_not_logged(self) -> None:
        server = WorkerServer(emit=EventLog())
        stderr = io.StringIO()
        with patch("sys.stderr", stderr):
            refused = server.handle(
                request("start_session", openai_api_key="sk-secret", transcription_model="gpt-5.4")
            )
            started = server.handle(
                request(
                    "start_session",
                    openai_api_key="sk-secret",
                    transcription_model="gpt-transcribe",
                )
            )

        self.assertFalse(refused.ok)
        self.assertEqual(refused.error.code, "bad_request")
        self.assertIn("Unsupported transcription model", refused.error.message)
        self.assertTrue(started.ok)
        self.assertNotIn("sk-secret", stderr.getvalue())


class NoAudioWorkerServerTests(unittest.TestCase):
    PROMPT = "Glossary:\nZoon\nZoon Robotics\nEuphoric"

    def server_returning(self, transcript: str) -> tuple[WorkerServer, FakeFileOpenAI]:
        server, _ = new_server(FakeFileOpenAI(transcript))
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


class SavedFileWorkerServerTests(unittest.TestCase):
    """The desktop hands the worker a saved recording's path instead of its audio."""

    def setUp(self) -> None:
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)

    def saved(self, name: str, content: bytes | None = None, pcm: bytes | None = None) -> str:
        path = os.path.join(self.folder.name, name)
        if pcm is not None:
            write_wav(path, pcm)
        else:
            with open(path, "wb") as target:
                target.write(content or b"")
        return path

    def transcribe_saved(self, server: WorkerServer, path: str, mime_type: str) -> tuple:
        session_id = server.handle(
            request("start_session", refine_enabled=False)
        ).result["session_id"]
        with patch("sys.stderr", io.StringIO()):
            finalized = server.handle(
                request(
                    "finalize_session_audio",
                    session_id=session_id,
                    mime_type=mime_type,
                    audio_path=path,
                )
            )
        return finalized, server._emit_sink.events(session_id)

    def test_saved_recording_is_read_in_place_and_kept(self) -> None:
        path = self.saved("audio.wav", pcm=pcm_tone(1.0))
        before = open(path, "rb").read()
        server, log = new_server(FakeFileOpenAI("Zoon Robotics is hiring."))

        finalized, events = self.transcribe_saved(server, path, "audio/wav")

        self.assertEqual(finalized.result, {"final_text": "Zoon Robotics is hiring."})
        self.assertEqual(events[-1], {"type": "final", "text": "Zoon Robotics is hiring."})
        self.assertEqual(server._openai.files, [(path, "audio/wav")])
        # The worker never copies or removes the desktop's saved file.
        self.assertEqual(open(path, "rb").read(), before)
        self.assertEqual(server._sessions, {})

    def test_saved_quick_tap_silence_and_empty_files_report_no_audio(self) -> None:
        cases = [
            self.saved("tap.wav", pcm=pcm_tone(0.1)),
            self.saved("silent.wav", pcm=bytes(24_000 * 2)),
            self.saved("empty.wav", content=b""),
            os.path.join(self.folder.name, "missing.wav"),
        ]
        for path in cases:
            server, _ = new_server(FakeFileOpenAI("Glossary: Zoon"))
            _, events = self.transcribe_saved(server, path, "audio/wav")
            self.assertEqual(events[-1], {"type": "error", "text": "No audio found"}, path)
            self.assertEqual(server._openai.transcribed, 0, path)

    def test_compressed_imports_go_straight_to_the_model(self) -> None:
        path = self.saved("memo.mp3", content=b"ID3 not really an mp3")
        server, _ = new_server(FakeFileOpenAI("A voice memo."))

        _, events = self.transcribe_saved(server, path, "audio/mpeg")

        self.assertEqual(events[-1], {"type": "final", "text": "A voice memo."})
        self.assertEqual(server._openai.files, [(path, "audio/mpeg")])

    def test_live_session_refuses_a_saved_file(self) -> None:
        path = self.saved("audio.wav", pcm=pcm_tone(1.0))
        openai = FakeRealtimeOpenAI()
        server, _ = new_server(openai)

        finalized, events = self.transcribe_saved(server, path, "audio/wav")

        self.assertEqual(finalized.result, {"final_text": ""})
        self.assertEqual(events[-1]["type"], "error")
        self.assertTrue(openai.stream.closed)
        self.assertEqual(server._sessions, {})


class RefineWorkerServerTests(unittest.TestCase):
    def dictate_with_refine(self, transcript: str, refine_prompt: str = "") -> tuple:
        openai = FakeFileOpenAI(transcript)
        server, _ = new_server(openai)
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
        server, _ = new_server(FakeFileOpenAI(
            self.DICTATED,
            polish_error=RuntimeError(f"Could not tidy: {self.DICTATED} {self.REFINE_PROMPT}"),
        ))
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
        server, _ = new_server(FakeFileOpenAI(
            "",
            transcribe_error=RuntimeError(f"Prompt rejected: {self.VOCABULARY}"),
        ))
        stderr = io.StringIO()

        with patch("sys.stderr", stderr):
            events = dictate(server, pcm_tone(1.0), self.VOCABULARY)

        self.assertEqual(events[-1], {"type": "error", "text": "Prompt rejected: [redacted]"})
        self.assertNotIn("Gwendolyn", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
