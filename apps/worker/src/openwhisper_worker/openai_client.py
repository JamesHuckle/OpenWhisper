from __future__ import annotations

import io
import os
from typing import Any

from openai import OpenAI


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
        extension = self._extension_for_mime(mime_type)
        filename = f"session_audio{extension}"
        audio_file = io.BytesIO(audio_bytes)
        audio_file.name = filename

        transcription = client.audio.transcriptions.create(
            model=self._model,
            file=audio_file,
        )
        text = getattr(transcription, "text", None)
        if not text:
            raise RuntimeError("Transcription API returned empty text")
        return text

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
