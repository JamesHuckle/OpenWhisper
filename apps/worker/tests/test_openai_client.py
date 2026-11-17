from __future__ import annotations

import io
import json
import os
import struct
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

import httpx
import openai

from openwhisper_worker.audio_rules import MAX_UPLOAD_BYTES, SILENT_PCM_PEAK
from openwhisper_worker.saved_parts import parts_key
from openwhisper_worker.openai_client import (
    OpenWhisperOpenAI,
    RealtimeTranscriptionStream,
    describe_error,
    is_short_clean_transcript,
)

DICTATED = "My bank PIN is four four seven one, please remind me at noon."
CUSTOM_PROMPT = "Write my mother's maiden name, Gwendolyn, in capitals."
TIDIED = "My bank PIN is 4471. Please remind me at noon."
# The bytes of one long-recording part: one upload less room for the WAV header.
PART_BYTES = (MAX_UPLOAD_BYTES - 128) // 2 * 2
SPEECH = 8_000


def responses_reply(text: str) -> SimpleNamespace:
    return SimpleNamespace(output=[SimpleNamespace(content=[SimpleNamespace(text=text)])])


def bad_request_quoting(text: str) -> openai.BadRequestError:
    response = httpx.Response(400, request=httpx.Request("POST", "https://api.openai.test/v1"))
    return openai.BadRequestError(
        f"Could not process input: {text}",
        response=response,
        body={"message": f"Could not process input: {text}", "code": "invalid_input"},
    )


def effort_rejected() -> openai.BadRequestError:
    response = httpx.Response(400, request=httpx.Request("POST", "https://api.openai.test/v1"))
    message = "Unsupported value: 'minimal' is not supported with this model."
    return openai.BadRequestError(
        message,
        response=response,
        body={"message": message, "param": "reasoning.effort", "code": "unsupported_value"},
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
            "Put this in capitals: hello.",
            "Make the next word bold please.",
            "New line thanks for coming.",
            "Buy apples, pears, plums and figs.",
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

    def test_a_recording_is_streamed_with_its_prompt(self) -> None:
        client = OpenWhisperOpenAI(api_key="sk-test", model="gpt-transcribe")
        api = Mock()
        api.audio.transcriptions.create.return_value.__enter__ = Mock(
            return_value=iter([SimpleNamespace(type="transcript.text.done", text="hello")])
        )
        api.audio.transcriptions.create.return_value.__exit__ = Mock(return_value=None)
        with patch.object(client, "_get_client", return_value=api):
            self.assertEqual(client.transcribe_bytes(b"RIFF", "audio/wav", "Glossary"), "hello")
        sent = api.audio.transcriptions.create.call_args.kwargs
        self.assertEqual(sent["model"], "gpt-transcribe")
        self.assertEqual(sent["prompt"], "Glossary")
        self.assertTrue(sent["stream"])
        self.assertEqual(sent["file"].name, "session_audio.wav")

    def test_flac_upload_is_named_as_flac(self) -> None:
        client = OpenWhisperOpenAI(api_key="sk-test", model="gpt-transcribe")
        api = Mock()
        with patch.object(client, "_get_client", return_value=api), patch.object(
            client, "_transcribe_streaming", return_value="hello"
        ) as transcribe:
            self.assertEqual(client.transcribe_bytes(b"fLaC", "audio/flac"), "hello")
        audio_file = transcribe.call_args.args[1]
        self.assertEqual(audio_file.name, "session_audio.flac")
        self.assertEqual(audio_file.getvalue(), b"fLaC")

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

    def test_sixty_minute_wav_is_transcribed_in_bounded_ordered_chunks(self) -> None:
        client = OpenWhisperOpenAI()
        pcm_bytes = 60 * 60 * 24_000 * 2
        progress: list[tuple[int, int]] = []
        uploaded_sizes: list[int] = []

        with tempfile.TemporaryDirectory() as directory:
            # A saved desktop recording: a 44-byte mono 16-bit 24 kHz WAV header
            # and an hour of sparse samples, with a loud one every megabyte.
            path = Path(directory) / "sixty-minutes.wav"
            with path.open("wb") as audio:
                audio.write(
                    b"RIFF"
                    + struct.pack("<I", 36 + pcm_bytes)
                    + b"WAVEfmt "
                    + struct.pack("<IHHIIHH", 16, 1, 1, 24_000, 24_000 * 2, 2, 16)
                    + b"data"
                    + struct.pack("<I", pcm_bytes)
                )
                audio.truncate(44 + pcm_bytes)
                for offset in range(0, pcm_bytes, 2**20):
                    audio.seek(44 + offset)
                    audio.write(struct.pack("<h", SPEECH))

            def fake_transcribe(wav: bytes, mime_type: str, prompt: str = "") -> str:
                self.assertEqual(mime_type, "audio/wav")
                self.assertLessEqual(len(wav), 24 * 1024 * 1024)
                self.assertEqual(wav[:4], b"RIFF")
                uploaded_sizes.append(len(wav))
                return f"segment-{len(uploaded_sizes)}"

            with patch.object(client, "transcribe_bytes", side_effect=fake_transcribe):
                transcript = client.transcribe_file(
                    path,
                    "audio/wav",
                    on_progress=lambda current, total: progress.append((current, total)),
                )

        self.assertGreater(len(uploaded_sizes), 1)
        self.assertEqual(sum(size - 44 for size in uploaded_sizes), pcm_bytes)
        self.assertEqual(
            transcript,
            " ".join(f"segment-{index}" for index in range(1, len(uploaded_sizes) + 1)),
        )
        self.assertEqual(progress, [(index, len(uploaded_sizes)) for index in range(1, len(uploaded_sizes) + 1)])

    def long_wav(
        self,
        directory: str,
        parts: int = 3,
        peaks: list[int] | None = None,
        last_part_bytes: int = 24_000 * 2,
    ) -> Path:
        """A saved desktop recording just long enough to be sent in `parts`
        parts. Each part is silence with one sample at its level in `peaks`
        (0 leaves it silent); by default every part holds speech."""
        peaks = peaks if peaks is not None else [SPEECH] * parts
        parts = len(peaks)
        pcm_bytes = (parts - 1) * PART_BYTES + last_part_bytes
        path = Path(directory) / "audio.wav"
        with path.open("wb") as audio:
            audio.write(
                b"RIFF"
                + struct.pack("<I", 36 + pcm_bytes)
                + b"WAVEfmt "
                + struct.pack("<IHHIIHH", 16, 1, 1, 24_000, 24_000 * 2, 2, 16)
                + b"data"
                + struct.pack("<I", pcm_bytes)
            )
            audio.truncate(44 + pcm_bytes)
            for index, peak in enumerate(peaks):
                if peak:
                    audio.seek(44 + index * PART_BYTES + 1_000)
                    audio.write(struct.pack("<h", -peak))
        return path

    def transcribe_long(
        self,
        client: OpenWhisperOpenAI,
        path: Path,
        replies: list[str | Exception],
        prompt: str = "Zoon",
    ) -> tuple[str, list[str]]:
        """Transcribes `path` with its parts saved beside it. Returns the text
        and the prompt each uploaded part was sent with."""
        prompts: list[str] = []
        pending = list(replies)

        def fake_transcribe(_wav: bytes, _mime_type: str, prompt: str = "") -> str:
            prompts.append(prompt)
            reply = pending.pop(0)
            if isinstance(reply, Exception):
                raise reply
            return reply

        with (
            patch.object(client, "transcribe_bytes", side_effect=fake_transcribe),
            patch("sys.stderr", io.StringIO()),
        ):
            text = client.transcribe_file(
                path, "audio/wav", prompt=prompt, parts_path=path.with_name("parts.json")
            )
        return text, prompts

    def test_a_retry_after_a_failed_last_part_sends_only_that_part(self) -> None:
        client = OpenWhisperOpenAI()
        with tempfile.TemporaryDirectory() as directory:
            path = self.long_wav(directory)
            with self.assertRaises(RuntimeError):
                self.transcribe_long(
                    client, path, ["First part.", "Second part.", RuntimeError("network")]
                )

            text, prompts = self.transcribe_long(client, path, ["Third part."])

        self.assertEqual(text, "First part. Second part. Third part.")
        # The last part's prompt is rebuilt from the saved text of the part before it.
        self.assertEqual(prompts, ["Zoon\nPrevious segment transcript: Second part."])

    def test_an_empty_part_that_finished_is_not_sent_again(self) -> None:
        client = OpenWhisperOpenAI()
        with tempfile.TemporaryDirectory() as directory:
            path = self.long_wav(directory)
            with self.assertRaises(RuntimeError):
                self.transcribe_long(client, path, ["First part.", "", RuntimeError("network")])

            text, prompts = self.transcribe_long(client, path, ["Third part."])

        self.assertEqual(text, "First part. Third part.")
        # The prompt quotes the last part that had text.
        self.assertEqual(prompts, ["Zoon\nPrevious segment transcript: First part."])

    def test_silent_parts_after_speech_are_never_uploaded(self) -> None:
        client = OpenWhisperOpenAI()
        with tempfile.TemporaryDirectory() as directory:
            path = self.long_wav(directory, peaks=[SPEECH, 0, 0])

            text, prompts = self.transcribe_long(client, path, ["Only part."])
            saved = json.loads(path.with_name("parts.json").read_text(encoding="utf-8"))
            # Saved as finished, so a retry uploads nothing either.
            retry_text, retry_prompts = self.transcribe_long(client, path, [])

        self.assertEqual(text, "Only part.")
        self.assertEqual(prompts, ["Zoon"])
        self.assertEqual(
            [(part["prompt"], part["text"]) for part in saved["parts"]],
            [("Zoon", "Only part."), ("", ""), ("", "")],
        )
        self.assertEqual((retry_text, retry_prompts), ("Only part.", []))

    def test_a_silent_part_between_speech_is_skipped_and_retries_skip_it(self) -> None:
        client = OpenWhisperOpenAI()
        with tempfile.TemporaryDirectory() as directory:
            path = self.long_wav(directory, peaks=[SPEECH, 0, SPEECH])
            with self.assertRaises(RuntimeError):
                self.transcribe_long(client, path, ["First part.", RuntimeError("network")])

            text, prompts = self.transcribe_long(client, path, ["Third part."])

        self.assertEqual(text, "First part. Third part.")
        # Only the failed part is sent again, prompted with the last text.
        self.assertEqual(prompts, ["Zoon\nPrevious segment transcript: First part."])

    def test_quiet_speech_and_a_short_last_part_are_still_uploaded(self) -> None:
        client = OpenWhisperOpenAI()
        with tempfile.TemporaryDirectory() as directory:
            # Part two never reaches the silence level; part one just does.
            # The last part is far shorter than a whole recording must be.
            path = self.long_wav(
                directory,
                peaks=[SILENT_PCM_PEAK, SILENT_PCM_PEAK - 1, SPEECH],
                last_part_bytes=24_000 * 2 // 10,
            )

            text, prompts = self.transcribe_long(client, path, ["Quiet.", "End."])

        self.assertEqual(text, "Quiet. End.")
        self.assertEqual(prompts, ["Zoon", "Zoon\nPrevious segment transcript: Quiet."])

    def test_parts_of_other_sample_formats_are_always_uploaded(self) -> None:
        client = OpenWhisperOpenAI()
        with tempfile.TemporaryDirectory() as directory:
            # 8-bit audio is not checked for silence: every part goes.
            pcm_bytes = PART_BYTES + 8_000
            path = Path(directory) / "audio.wav"
            with path.open("wb") as audio:
                audio.write(
                    b"RIFF"
                    + struct.pack("<I", 36 + pcm_bytes)
                    + b"WAVEfmt "
                    + struct.pack("<IHHIIHH", 16, 1, 1, 8_000, 8_000, 1, 8)
                    + b"data"
                    + struct.pack("<I", pcm_bytes)
                )
                audio.truncate(44 + pcm_bytes)

            text, prompts = self.transcribe_long(client, path, ["One.", "Two."])

        self.assertEqual(text, "One. Two.")
        self.assertEqual(len(prompts), 2)

    def test_changed_settings_or_audio_start_the_parts_afresh(self) -> None:
        def changed_prompt(_client: OpenWhisperOpenAI, _path: Path) -> dict:
            return {"prompt": "Zoon Robotics"}

        def changed_audio(_client: OpenWhisperOpenAI, path: Path) -> dict:
            os.utime(path, ns=(0, path.stat().st_mtime_ns + 1_000_000_000))
            return {}

        for change in (changed_prompt, changed_audio):
            client = OpenWhisperOpenAI()
            with tempfile.TemporaryDirectory() as directory:
                path = self.long_wav(directory)
                with self.assertRaises(RuntimeError):
                    self.transcribe_long(client, path, ["One.", "Two.", RuntimeError("network")])
                options = change(client, path)

                text, prompts = self.transcribe_long(
                    client, path, ["Uno.", "Dos.", "Tres."], **options
                )

            self.assertEqual(text, "Uno. Dos. Tres.", change.__name__)
            self.assertEqual(len(prompts), 3, change.__name__)

        # Only one model reads saved files today; another one gets its own key.
        with tempfile.TemporaryDirectory() as directory:
            path = self.long_wav(directory, parts=1)
            self.assertNotEqual(
                parts_key(path, "gpt-transcribe", "Zoon"),
                parts_key(path, "gpt-transcribe-next", "Zoon"),
            )

    def test_a_damaged_parts_file_starts_afresh(self) -> None:
        client = OpenWhisperOpenAI()
        with tempfile.TemporaryDirectory() as directory:
            path = self.long_wav(directory)
            path.with_name("parts.json").write_text("{not json", encoding="utf-8")

            text, prompts = self.transcribe_long(client, path, ["One.", "Two.", "Three."])

        self.assertEqual(text, "One. Two. Three.")
        self.assertEqual(len(prompts), 3)

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

    def test_polish_asks_for_minimal_reasoning(self) -> None:
        client = OpenWhisperOpenAI()
        client._client = Mock()
        client._client.responses.create.return_value = responses_reply(TIDIED)
        stderr = io.StringIO()

        with patch("sys.stderr", stderr):
            self.assertEqual(client.polish(DICTATED), TIDIED)

        sent = client._client.responses.create.call_args.kwargs
        self.assertEqual(sent["reasoning"], {"effort": "minimal"})
        self.assertIn("effort=minimal", stderr.getvalue())

    def test_rejected_minimal_reasoning_retries_once_with_low_and_remembers_it(self) -> None:
        client = OpenWhisperOpenAI()
        client._client = Mock()
        client._client.responses.create.side_effect = [
            effort_rejected(),
            responses_reply(TIDIED),
            responses_reply(TIDIED),
        ]
        stderr = io.StringIO()

        with patch("sys.stderr", stderr):
            self.assertEqual(client.polish(DICTATED), TIDIED)
            self.assertEqual(client.polish(DICTATED), TIDIED)

        efforts = [
            call.kwargs["reasoning"]["effort"]
            for call in client._client.responses.create.call_args_list
        ]
        self.assertEqual(efforts, ["minimal", "low", "low"])
        log = stderr.getvalue()
        self.assertEqual(log.count("effort_fallback"), 1)
        self.assertNotIn("bank PIN", log)
        self.assertIn("effort=low", log)

    def test_an_overridden_tidy_model_is_asked_through_the_responses_api(self) -> None:
        with patch.dict(os.environ, {"OPENWHISPER_POLISH_MODEL": "gpt-5.4"}, clear=True):
            client = OpenWhisperOpenAI()
        client._client = Mock()
        client._client.responses.create.return_value = responses_reply(TIDIED)

        with patch("sys.stderr", io.StringIO()):
            self.assertEqual(client.polish(DICTATED), TIDIED)

        self.assertEqual(client._client.responses.create.call_args.kwargs["model"], "gpt-5.4")
        client._client.chat.completions.create.assert_not_called()

    def test_rejected_low_reasoning_is_not_retried(self) -> None:
        client = OpenWhisperOpenAI()
        client._client = Mock()
        client._client.responses.create.side_effect = effort_rejected()

        with patch("sys.stderr", io.StringIO()):
            with self.assertRaises(openai.BadRequestError):
                client.polish(DICTATED)

        self.assertEqual(client._client.responses.create.call_count, 2)

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
