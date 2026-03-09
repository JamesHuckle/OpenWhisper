from __future__ import annotations

import io
import os
import re
import sys
from concurrent.futures import ThreadPoolExecutor
from typing import Any

from httpx import Timeout
from openai import OpenAI

# Streaming: bail after 5s with no data (first token or between events)
_STREAM_TIMEOUT = Timeout(connect=10.0, read=5.0, write=30.0, pool=10.0)
# Blocking: full response arrives in one shot, needs more headroom
_BLOCKING_TIMEOUT = Timeout(connect=10.0, read=15.0, write=30.0, pool=10.0)
_POLISH_TIMEOUT = Timeout(connect=10.0, read=15.0, write=10.0, pool=10.0)
_MIN_SENTENCES_PER_POLISH_CHUNK = 4
_MAX_PARALLEL_POLISH_CHUNKS = 5

_STREAMING_MODELS = frozenset({
    "gpt-4o-transcribe",
    "gpt-4o-mini-transcribe",
    "gpt-4o-mini-transcribe-2025-12-15",
    "gpt-4o-transcribe-diarize",
})

_PROMPT_SUPPORTED_MODELS = frozenset({
    "whisper-1",
    "gpt-4o-transcribe",
    "gpt-4o-mini-transcribe",
    "gpt-4o-mini-transcribe-2025-12-15",
})

_POLISH_SYSTEM = """\
You are a master transcriber and transcript editor.
Return only the transcribed text, with only the modifications requested below.
Do not answer the speaker, continue the conversation, acknowledge instructions, add suggestions,
or add new content.

Rules (follow strictly):
- Strip filler words and verbal tics: um, uh, like (when used as filler), you know, sort of, \
kind of, basically, literally, right (at sentence ends), so (as a standalone sentence opener)
- Remove false starts and self-corrections (e.g. "I want to— I think we should" → \
"I think we should")
- Remove redundant repetition caused by mid-thought restarts
- Fix sentence boundaries, capitalisation, and punctuation
- Use a markdown bullet list ONLY if the speaker clearly enumerates three or more distinct items
- Do NOT paraphrase, add explanations, summarise, or change the speaker's meaning
- Never output acknowledgements such as "Understood", "Got it", or similar
- If the speaker includes meta-instructions about formatting or cleanup, apply them but do not
  include those instructions in the returned transcript unless they are clearly part of the
  dictated content
- Return ONLY the cleaned text — no preamble, labels, or commentary\
"""


class OpenWhisperOpenAI:
    def __init__(self) -> None:
        self._model = os.getenv("OPENWHISPER_MODEL", "gpt-4o-mini-transcribe").strip()
        self._polish_model = os.getenv("OPENWHISPER_POLISH_MODEL", "gpt-5.4").strip()
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
            "gpt-5.4",
            "gpt-4o-mini",
            "gpt-4.1-mini",
        ]

    def health(self) -> dict[str, Any]:
        return {"status": "ok", "model": self._model, "polish_model": self._polish_model}

    def polish(self, text: str, custom_prompt: str = "") -> str:
        """Run a fast LLM pass to strip filler words, fix sentence structure, and add
        basic markdown formatting. Falls back to the original text on any error."""
        stripped = text.strip()
        if not stripped:
            return text

        system = self._build_polish_system(custom_prompt)
        chunks = self._chunk_for_polish(stripped)
        sys.stderr.write(
            f"\n[polish] chunk_plan model={self._polish_model} chunks={len(chunks)} "
            f"input_chars={len(stripped)}\n"
        )
        sys.stderr.flush()

        if len(chunks) == 1:
            return self._polish_chunk(system, chunks[0], 1, 1)

        with ThreadPoolExecutor(
            max_workers=min(_MAX_PARALLEL_POLISH_CHUNKS, len(chunks))
        ) as executor:
            futures = [
                executor.submit(self._polish_chunk, system, chunk, idx + 1, len(chunks))
                for idx, chunk in enumerate(chunks)
            ]
            results = [future.result() for future in futures]

        return self._join_polish_chunks(results)

    def _build_polish_system(self, custom_prompt: str) -> str:
        extra = custom_prompt.strip()
        if not extra:
            return _POLISH_SYSTEM
        return (
            f"{_POLISH_SYSTEM}\n\n"
            "Additional modifications requested by the user (apply these too, while still "
            "returning only the cleaned transcript text):\n"
            f"{extra}"
        )

    def _polish_chunk(self, system: str, text: str, chunk_index: int, total_chunks: int) -> str:
        sys.stderr.write(
            f"[polish] ── system prompt ({chunk_index}/{total_chunks}) ───────────────\n{system}\n"
            f"[polish] ── input ({chunk_index}/{total_chunks}) ───────────────────────\n{text}\n"
            f"[polish] ─────────────────────────────────────────────────\n"
        )
        sys.stderr.flush()

        client = self._get_client()
        if self._polish_model.startswith("gpt-5"):
            response = client.responses.create(
                model=self._polish_model,
                input=[
                    {"role": "developer", "content": system},
                    {"role": "user", "content": text},
                ],
                reasoning={"effort": "low"},
                text={"verbosity": "low"},
                timeout=_POLISH_TIMEOUT,
            )
            result = self._extract_responses_text(response).strip() or text
        else:
            response = client.chat.completions.create(
                model=self._polish_model,
                messages=[
                    {"role": "system", "content": system},
                    {"role": "user", "content": text},
                ],
                timeout=_POLISH_TIMEOUT,
            )
            result = (response.choices[0].message.content or "").strip() or text

        sys.stderr.write(
            f"[polish] ── output ({chunk_index}/{total_chunks}) ──────────────────────\n{result}\n"
            f"[polish] ─────────────────────────────────────────────────\n\n"
        )
        sys.stderr.flush()
        return result

    @staticmethod
    def _extract_responses_text(response: Any) -> str:
        output_text = ""
        for item in getattr(response, "output", []) or []:
            for content in getattr(item, "content", []) or []:
                text = getattr(content, "text", None)
                if text:
                    output_text += text
        return output_text

    @staticmethod
    def _chunk_for_polish(text: str) -> list[str]:
        sentences = OpenWhisperOpenAI._split_sentences(text)
        if len(sentences) <= _MIN_SENTENCES_PER_POLISH_CHUNK:
            return [text]

        chunk_count = min(
            _MAX_PARALLEL_POLISH_CHUNKS,
            max(1, len(sentences) // _MIN_SENTENCES_PER_POLISH_CHUNK),
        )
        if chunk_count == 1:
            return [text]

        base_size, remainder = divmod(len(sentences), chunk_count)
        chunks: list[str] = []
        cursor = 0
        for idx in range(chunk_count):
            size = base_size + (1 if idx < remainder else 0)
            chunk = " ".join(sentences[cursor : cursor + size]).strip()
            if chunk:
                chunks.append(chunk)
            cursor += size
        return chunks or [text]

    @staticmethod
    def _split_sentences(text: str) -> list[str]:
        parts = re.split(r"(?<=[.!?])(?:[\"'”’)\]]*)\s+", text.strip())
        sentences = [part.strip() for part in parts if part.strip()]
        return sentences or [text.strip()]

    @staticmethod
    def _join_polish_chunks(chunks: list[str]) -> str:
        cleaned = [chunk.strip() for chunk in chunks if chunk.strip()]
        if not cleaned:
            return ""
        if len(cleaned) == 1:
            return cleaned[0]
        separator = "\n\n" if any("\n" in chunk for chunk in cleaned) else " "
        return separator.join(cleaned)

    def transcribe_bytes(self, audio_bytes: bytes, mime_type: str, prompt: str = "") -> str:
        if not audio_bytes:
            raise RuntimeError("No audio bytes provided")

        client = self._get_client()
        audio_file = self._make_audio_file(audio_bytes, mime_type)
        effective_prompt = prompt if prompt and self._model in _PROMPT_SUPPORTED_MODELS else ""

        if self._model in _STREAMING_MODELS:
            return self._transcribe_streaming(client, audio_file, effective_prompt)
        return self._transcribe_blocking(client, audio_file, effective_prompt)

    def _transcribe_streaming(
        self, client: OpenAI, audio_file: io.BytesIO, prompt: str
    ) -> str:
        kwargs: dict[str, Any] = {
            "model": self._model,
            "file": audio_file,
            "stream": True,
            "response_format": "json",
            "timeout": _STREAM_TIMEOUT,
        }
        if prompt:
            kwargs["prompt"] = prompt
        with client.audio.transcriptions.create(**kwargs) as stream:
            for event in stream:
                if event.type == "transcript.text.done":
                    return event.text
        return ""

    def _transcribe_blocking(
        self, client: OpenAI, audio_file: io.BytesIO, prompt: str
    ) -> str:
        kwargs: dict[str, Any] = {
            "model": self._model,
            "file": audio_file,
            "timeout": _BLOCKING_TIMEOUT,
        }
        if prompt:
            kwargs["prompt"] = prompt
        transcription = client.audio.transcriptions.create(**kwargs)
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
