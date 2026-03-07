from __future__ import annotations

import io
import os
from typing import Any

from httpx import Timeout
from openai import OpenAI

# Streaming: bail after 5s with no data (first token or between events)
_STREAM_TIMEOUT = Timeout(connect=10.0, read=5.0, write=30.0, pool=10.0)
# Blocking: full response arrives in one shot, needs more headroom
_BLOCKING_TIMEOUT = Timeout(connect=10.0, read=15.0, write=30.0, pool=10.0)

_STREAMING_MODELS = frozenset({
    "gpt-4o-transcribe",
    "gpt-4o-mini-transcribe",
    "gpt-4o-mini-transcribe-2025-12-15",
    "gpt-4o-transcribe-diarize",
})


class OpenWhisperOpenAI:
    def __init__(self) -> None:
        self._model = os.getenv("OPENWHISPER_MODEL", "gpt-4o-mini-transcribe").strip()
        self._client: OpenAI | None = None

    @property
    def model(self) -> str:
        return self._model

    def _get_client(self) -> OpenAI:
        if self._client is None:
            api_key = (os.getenv("OPENAI_API_KEY") or "").strip()
            if not api_key:
                raise RuntimeError("OPENAI_API_KEY is not set")
            self._client = OpenAI(api_key=api_key)
        return self._client

    def available_models(self) -> list[str]:
        return [
            "gpt-4o-mini-transcribe",
            "gpt-4o-transcribe",
            "gpt-4o-mini",
            "gpt-4.1-mini",
        ]

    def health(self) -> dict[str, Any]:
        return {"status": "ok", "model": self._model}

    def transcribe_bytes(self, audio_bytes: bytes, mime_type: str) -> str:
        if not audio_bytes:
            raise RuntimeError("No audio bytes provided")

        client = self._get_client()
        audio_file = self._make_audio_file(audio_bytes, mime_type)

        if self._model in _STREAMING_MODELS:
            return self._transcribe_streaming(client, audio_file)
        return self._transcribe_blocking(client, audio_file)

    def _transcribe_streaming(self, client: OpenAI, audio_file: io.BytesIO) -> str:
        with client.audio.transcriptions.create(
            model=self._model,
            file=audio_file,
            stream=True,
            response_format="json",
            timeout=_STREAM_TIMEOUT,
        ) as stream:
            for event in stream:
                if event.type == "transcript.text.done":
                    return event.text
        return ""

    def _transcribe_blocking(self, client: OpenAI, audio_file: io.BytesIO) -> str:
        transcription = client.audio.transcriptions.create(
            model=self._model,
            file=audio_file,
            timeout=_BLOCKING_TIMEOUT,
        )
        return getattr(transcription, "text", "") or ""

    def _make_audio_file(self, audio_bytes: bytes, mime_type: str) -> io.BytesIO:
        extension = self._extension_for_mime(mime_type)
        audio_file = io.BytesIO(audio_bytes)
        audio_file.name = f"session_audio{extension}"
        return audio_file

    @staticmethod
    def _extension_for_mime(mime_type: str) -> str:
        mime_map = {
            "audio/webm": ".webm",
            "audio/wav": ".wav",
            "audio/mpeg": ".mp3",
            "audio/mp4": ".m4a",
            "audio/ogg": ".ogg",
        }
        return mime_map.get(mime_type, ".webm")
