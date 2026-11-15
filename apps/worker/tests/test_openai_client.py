from __future__ import annotations

import io
import os
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

import httpx
import openai

from openwhisper_worker.openai_client import (
    OpenWhisperOpenAI,
    RealtimeTranscriptionStream,
    describe_error,
    is_short_clean_transcript,
)

DICTATED = "My bank PIN is four four seven one, please remind me at noon."
CUSTOM_PROMPT = "Write my mother's maiden name, Gwendolyn, in capitals."
TIDIED = "My bank PIN is 4471. Please remind me at noon."


def responses_reply(text: str) -> SimpleNamespace:
    return SimpleNamespace(output=[SimpleNamespace(content=[SimpleNamespace(text=text)])])


def bad_request_quoting(text: str) -> openai.BadRequestError:
    response = httpx.Response(400, request=httpx.Request("POST", "https://api.openai.test/v1"))
    return openai.BadRequestError(
        f"Could not process input: {text}",
        response=response,
        body={"message": f"Could not process input: {text}", "code": "invalid_input"},
    )


class OpenWhisperOpenAITests(unittest.TestCase):
    def test_realtime_stream_uses_session_model_and_emits_deltas(self) -> None:
        connection = Mock()
        connection.__iter__ = Mock(
            return_value=iter(
                [
                    SimpleNamespace(
                        type="conversation.item.input_audio_transcription.delta",
                        delta="hello",
                    ),
                    SimpleNamespace(
                        type="conversation.item.input_audio_transcription.completed",
                        transcript="hello world",
                    ),
                ]
            )
        )
        manager = Mock()
        manager.__enter__ = Mock(return_value=connection)
        manager.__exit__ = Mock(return_value=None)
        client = Mock()
        client.realtime.connect.return_value = manager
        deltas: list[str] = []

        stream = RealtimeTranscriptionStream(client, deltas.append)
        stream.append(b"\x01\x02")
        self.assertEqual(stream.finalize(), "hello world")

        client.realtime.connect.assert_called_once_with(
            extra_query={"intent": "transcription"},
            websocket_connection_options={"open_timeout": 10, "close_timeout": 5},
        )
        session = connection.session.update.call_args.kwargs["session"]
        self.assertEqual(session["type"], "transcription")
        self.assertNotIn("output_modalities", session)
        self.assertEqual(
            session["audio"]["input"]["transcription"]["model"],
            "gpt-live-transcribe",
        )
        self.assertEqual(deltas, ["hello"])

    def test_gpt_transcribe_is_the_default(self) -> None:
        with patch.dict(os.environ, {}, clear=True):
            client = OpenWhisperOpenAI()
        self.assertEqual(client.model, "gpt-transcribe")
        self.assertFalse(client.is_live)

    def test_small_model_tidies_transcripts_by_default(self) -> None:
        with patch.dict(os.environ, {}, clear=True):
            client = OpenWhisperOpenAI()
        self.assertEqual(client.health()["polish_model"], "gpt-5-nano")
        with patch.dict(os.environ, {"OPENWHISPER_POLISH_MODEL": "gpt-5.4"}, clear=True):
            self.assertEqual(OpenWhisperOpenAI().health()["polish_model"], "gpt-5.4")

    def test_short_clean_transcripts_are_recognised(self) -> None:
        for text in (
            "Send me the report by Friday.",
            "  Can you call me back?  ",
            "I like it, thanks!",
            "3 people are coming.",
            "Meet me at 3.30 today.",
        ):
            with self.subTest(text=text):
                self.assertTrue(is_short_clean_transcript(text))
        for text in (
            "",
            "send me the report by friday.",
            "Send me the report by Friday",
            "Send me the report. Thanks.",
            "Send it now\nThanks.",
            "Please send me the full quarterly sales report and the budget by Friday.",
            "Um, send me the report.",
            "Uh send me the report.",
            "Send me, you know, the report.",
            "Send me the, like, report.",
            "Like, send me the report.",
            "So send me the report.",
            "It is basically done.",
            "Send me the report, right?",
            "I want\u2014 I think we should go.",
            "I want -- I think we should go.",
            "Send me the the report.",
        ):
            with self.subTest(text=text):
                self.assertFalse(is_short_clean_transcript(text))

    def test_available_models_only_contains_transcription_models(self) -> None:
        client = OpenWhisperOpenAI()
        self.assertEqual(
            client.available_models(),
            ["gpt-transcribe", "gpt-live-transcribe"],
        )

    def test_legacy_saved_models_map_to_current_models(self) -> None:
        for legacy, current in (
            ("gpt-4o-mini-transcribe", "gpt-transcribe"),
            ("gpt-4o-transcribe", "gpt-transcribe"),
            ("gpt-realtime-transcribe", "gpt-live-transcribe"),
        ):
            with patch.dict(os.environ, {"OPENWHISPER_MODEL": legacy}):
                self.assertEqual(OpenWhisperOpenAI().model, current)

    def test_live_transcribe_routes_pcm_to_realtime_api(self) -> None:
        with patch.dict(os.environ, {"OPENWHISPER_MODEL": "gpt-live-transcribe"}):
            client = OpenWhisperOpenAI()
        stream = Mock()
        stream.finalize.return_value = "hello"
        with patch.object(client, "start_realtime_transcription", return_value=stream) as start:
            result = client.transcribe_bytes(b"\x00\x00", "audio/pcm;rate=24000", "Glossary")
        self.assertEqual(result, "hello")
        start.assert_called_once()
        stream.append.assert_called_once_with(b"\x00\x00")
        stream.finalize.assert_called_once_with()

    def test_live_transcribe_rejects_compressed_audio(self) -> None:
        with patch.dict(os.environ, {"OPENWHISPER_MODEL": "gpt-live-transcribe"}):
            client = OpenWhisperOpenAI()
        with self.assertRaisesRegex(RuntimeError, "requires 24 kHz PCM"):
            client.transcribe_bytes(b"webm", "audio/webm")

    def test_a_given_key_and_model_win_over_the_environment(self) -> None:
        environment = {"OPENAI_API_KEY": "env-key", "OPENWHISPER_MODEL": "gpt-transcribe"}
        with patch.dict(os.environ, environment, clear=True):
            client = OpenWhisperOpenAI(api_key=" saved-key ", model="gpt-live-transcribe")
            with patch("openwhisper_worker.openai_client.OpenAI") as make_client:
                client._get_client()
        self.assertEqual(client.model, "gpt-live-transcribe")
        make_client.assert_called_once_with(api_key="saved-key")

    def test_an_empty_key_and_model_fall_back_to_the_environment(self) -> None:
        environment = {"OPENAI_API_KEY": "env-key", "OPENWHISPER_MODEL": "gpt-4o-transcribe"}
        with patch.dict(os.environ, environment, clear=True):
            client = OpenWhisperOpenAI(api_key="", model="")
            with patch("openwhisper_worker.openai_client.OpenAI") as make_client:
                client._get_client()
        self.assertEqual(client.model, "gpt-transcribe")
        make_client.assert_called_once_with(api_key="env-key")

    def test_unknown_transcription_model_is_rejected(self) -> None:
        with patch.dict(os.environ, {"OPENWHISPER_MODEL": "gpt-5.4"}):
            with self.assertRaisesRegex(ValueError, "Unsupported transcription model"):
                OpenWhisperOpenAI()

    def test_sixty_minute_pcm_is_transcribed_in_bounded_ordered_chunks(self) -> None:
        client = OpenWhisperOpenAI()
        pcm_bytes = 60 * 60 * 24_000 * 2
        progress: list[tuple[int, int]] = []
        uploaded_sizes: list[int] = []

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "sixty-minutes.pcm"
            with path.open("wb") as audio:
                audio.truncate(pcm_bytes)

            def fake_transcribe(wav: bytes, mime_type: str, prompt: str = "") -> str:
                self.assertEqual(mime_type, "audio/wav")
                self.assertLessEqual(len(wav), 24 * 1024 * 1024)
                self.assertEqual(wav[:4], b"RIFF")
                uploaded_sizes.append(len(wav))
                return f"segment-{len(uploaded_sizes)}"

            with patch.object(client, "transcribe_bytes", side_effect=fake_transcribe):
                transcript = client.transcribe_file(
                    path,
                    "audio/pcm;rate=24000",
                    on_progress=lambda current, total: progress.append((current, total)),
                )

        self.assertGreater(len(uploaded_sizes), 1)
        self.assertEqual(sum(size - 44 for size in uploaded_sizes), pcm_bytes)
        self.assertEqual(
            transcript,
            " ".join(f"segment-{index}" for index in range(1, len(uploaded_sizes) + 1)),
        )
        self.assertEqual(progress, [(index, len(uploaded_sizes)) for index in range(1, len(uploaded_sizes) + 1)])

    def test_polish_logs_counts_and_timings_but_never_the_text(self) -> None:
        client = OpenWhisperOpenAI()
        client._client = Mock()
        client._client.responses.create.return_value = responses_reply(TIDIED)
        stderr = io.StringIO()

        with patch("sys.stderr", stderr):
            result = client.polish(DICTATED, CUSTOM_PROMPT)

        self.assertEqual(result, TIDIED)
        sent = client._client.responses.create.call_args.kwargs["input"]
        self.assertIn(CUSTOM_PROMPT, sent[0]["content"])
        self.assertEqual(sent[1]["content"], DICTATED)
        log = stderr.getvalue()
        for private in (DICTATED, CUSTOM_PROMPT, TIDIED, "bank PIN", "Gwendolyn"):
            self.assertNotIn(private, log)
        self.assertNotIn("master transcriber", log)
        self.assertIn(f"input_chars={len(DICTATED)}", log)
        self.assertIn(f"output_chars={len(TIDIED)}", log)
        self.assertIn("elapsed_ms=", log)

    def test_failed_polish_logs_error_codes_but_never_the_text(self) -> None:
        client = OpenWhisperOpenAI()
        client._client = Mock()
        client._client.responses.create.side_effect = bad_request_quoting(DICTATED)
        stderr = io.StringIO()

        with patch("sys.stderr", stderr):
            with self.assertRaises(openai.BadRequestError):
                client.polish(DICTATED, CUSTOM_PROMPT)

        log = stderr.getvalue()
        self.assertNotIn("bank PIN", log)
        self.assertNotIn("Gwendolyn", log)
        self.assertIn("error=BadRequestError status=400 code=invalid_input", log)

    def test_describe_error_leaves_out_the_message(self) -> None:
        self.assertEqual(
            describe_error(bad_request_quoting(DICTATED)),
            "BadRequestError status=400 code=invalid_input",
        )
        self.assertEqual(describe_error(RuntimeError(DICTATED)), "RuntimeError")


if __name__ == "__main__":
    unittest.main()
