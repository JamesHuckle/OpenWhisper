from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

from openwhisper_worker.openai_client import OpenWhisperOpenAI, RealtimeTranscriptionStream


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
            "gpt-realtime-whisper",
        )
        self.assertEqual(deltas, ["hello"])

    def test_mini_transcribe_is_the_default(self) -> None:
        with patch.dict(os.environ, {}, clear=True):
            client = OpenWhisperOpenAI()
        self.assertEqual(client.model, "gpt-4o-mini-transcribe")

    def test_available_models_only_contains_transcription_models(self) -> None:
        client = OpenWhisperOpenAI()
        self.assertEqual(
            client.available_models(),
            [
                "gpt-4o-mini-transcribe",
                "gpt-4o-transcribe",
                "gpt-realtime-transcribe",
            ],
        )

    def test_realtime_transcribe_alias_is_supported(self) -> None:
        with patch.dict(os.environ, {"OPENWHISPER_MODEL": "gpt-realtime-transcribe"}):
            client = OpenWhisperOpenAI()
        self.assertEqual(client.model, "gpt-realtime-transcribe")

    def test_realtime_transcribe_routes_pcm_to_realtime_api(self) -> None:
        with patch.dict(os.environ, {"OPENWHISPER_MODEL": "gpt-realtime-transcribe"}):
            client = OpenWhisperOpenAI()
        stream = Mock()
        stream.finalize.return_value = "hello"
        with patch.object(client, "start_realtime_transcription", return_value=stream) as start:
            result = client.transcribe_bytes(b"\x00\x00", "audio/pcm;rate=24000", "Glossary")
        self.assertEqual(result, "hello")
        start.assert_called_once()
        stream.append.assert_called_once_with(b"\x00\x00")
        stream.finalize.assert_called_once_with()

    def test_realtime_transcribe_rejects_compressed_audio(self) -> None:
        with patch.dict(os.environ, {"OPENWHISPER_MODEL": "gpt-realtime-transcribe"}):
            client = OpenWhisperOpenAI()
        with self.assertRaisesRegex(RuntimeError, "requires 24 kHz PCM"):
            client.transcribe_bytes(b"webm", "audio/webm")

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


if __name__ == "__main__":
    unittest.main()
