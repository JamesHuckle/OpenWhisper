from __future__ import annotations

import base64
import io
import os
import re
import sys
import threading
import wave
from concurrent.futures import ThreadPoolExecutor
from collections.abc import Callable
from pathlib import Path
from typing import Any

from httpx import Timeout
from openai import OpenAI

# Long recordings can legitimately take many minutes to upload and transcribe.
_STREAM_TIMEOUT = Timeout(connect=30.0, read=1800.0, write=600.0, pool=30.0)
_BLOCKING_TIMEOUT = Timeout(connect=30.0, read=1800.0, write=600.0, pool=30.0)
_POLISH_TIMEOUT = Timeout(connect=30.0, read=300.0, write=120.0, pool=30.0)
_MIN_SENTENCES_PER_POLISH_CHUNK = 4
_MAX_PARALLEL_POLISH_CHUNKS = 5
_MAX_POLISH_CHUNK_CHARS = 12_000
_MAX_AUDIO_UPLOAD_BYTES = 24 * 1024 * 1024
_REALTIME_TRANSCRIBE_ALIAS = "gpt-realtime-transcribe"
_REALTIME_TRANSCRIBE_MODEL = "gpt-realtime-whisper"
_REALTIME_FINAL_TIMEOUT_SECONDS = 120.0

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

_TRANSCRIPTION_MODELS = (
    "gpt-4o-mini-transcribe",
    "gpt-4o-transcribe",
    _REALTIME_TRANSCRIBE_ALIAS,
)


class RealtimeTranscriptionStream:
    """A persistent Realtime connection that accepts PCM while recording."""

    def __init__(
        self,
        client: OpenAI,
        on_delta: Callable[[str], None],
    ) -> None:
        self._on_delta = on_delta
        self._manager = client.realtime.connect(
            # The dedicated transcription transport is selected by intent. It
            # rejects both a conversational session model and a model query;
            # the transcription model belongs in session.audio.input below.
            extra_query={"intent": "transcription"},
            websocket_connection_options={"open_timeout": 10, "close_timeout": 5},
        )
        self._connection = self._manager.__enter__()
        self._completed = threading.Event()
        self._deltas: list[str] = []
        self._transcript = ""
        self._error: str | None = None
        self._closed = False

        self._connection.session.update(
            session={
                "type": "transcription",
                "audio": {
                    "input": {
                        "format": {"type": "audio/pcm", "rate": 24000},
                        "transcription": {
                            "model": _REALTIME_TRANSCRIBE_MODEL,
                            "delay": "minimal",
                        },
                        "turn_detection": None,
                    }
                },
            }
        )
        self._reader = threading.Thread(target=self._read_events, daemon=True)
        self._reader.start()

    def append(self, audio_bytes: bytes) -> None:
        if self._closed:
            raise RuntimeError("Realtime transcription session is closed")
        if audio_bytes:
            self._connection.input_audio_buffer.append(
                audio=base64.b64encode(audio_bytes).decode("ascii")
            )

    def finalize(self, timeout: float = _REALTIME_FINAL_TIMEOUT_SECONDS) -> str:
        if self._closed:
            raise RuntimeError("Realtime transcription session is closed")
        self._connection.input_audio_buffer.commit()
        if not self._completed.wait(timeout):
            self.close()
            raise RuntimeError("Realtime transcription timed out")
        try:
            if self._error:
                raise RuntimeError(self._error)
            return self._transcript or "".join(self._deltas)
        finally:
            self.close()

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        self._manager.__exit__(None, None, None)

    def _read_events(self) -> None:
        try:
            for event in self._connection:
                if event.type == "conversation.item.input_audio_transcription.delta":
                    delta = getattr(event, "delta", None)
                    if delta:
                        self._deltas.append(delta)
                        self._on_delta(delta)
                elif event.type == "conversation.item.input_audio_transcription.completed":
                    self._transcript = getattr(event, "transcript", "") or ""
                    self._completed.set()
                    return
                elif event.type == "conversation.item.input_audio_transcription.failed":
                    error = getattr(event, "error", None)
                    self._error = (
                        getattr(error, "message", None) or "Realtime transcription failed"
                    )
                    self._completed.set()
                    return
                elif event.type == "error":
                    error = getattr(event, "error", None)
                    self._error = getattr(error, "message", None) or "Realtime API error"
                    self._completed.set()
                    return
        except Exception as exc:  # noqa: BLE001
            if not self._closed:
                self._error = str(exc)
                self._completed.set()

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
        if self._model not in _TRANSCRIPTION_MODELS:
            raise ValueError(f"Unsupported transcription model: {self._model}")
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
        return list(_TRANSCRIPTION_MODELS)

    def health(self) -> dict[str, Any]:
        return {"status": "ok", "model": self._model, "polish_model": self._polish_model}

    def start_realtime_transcription(
        self, on_delta: Callable[[str], None]
    ) -> RealtimeTranscriptionStream:
        if self._model != _REALTIME_TRANSCRIBE_ALIAS:
            raise RuntimeError("The selected model is not a realtime transcription model")
        return RealtimeTranscriptionStream(self._get_client(), on_delta)

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
        if (
            len(sentences) <= _MIN_SENTENCES_PER_POLISH_CHUNK
            and len(text) <= _MAX_POLISH_CHUNK_CHARS
        ):
            return [text]

        chunks: list[str] = []
        current: list[str] = []
        current_chars = 0
        for sentence in sentences:
            added_chars = len(sentence) + (1 if current else 0)
            if current and current_chars + added_chars > _MAX_POLISH_CHUNK_CHARS:
                chunks.append(" ".join(current))
                current = []
                current_chars = 0
            current.append(sentence)
            current_chars += len(sentence) + (1 if len(current) > 1 else 0)
        if current:
            chunks.append(" ".join(current))
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

        if self._model == _REALTIME_TRANSCRIBE_ALIAS:
            if not mime_type.startswith("audio/pcm"):
                raise RuntimeError("Realtime Transcribe requires 24 kHz PCM audio")
            stream = self.start_realtime_transcription(lambda _delta: None)
            try:
                stream.append(audio_bytes)
                return stream.finalize()
            except Exception:
                stream.close()
                raise

        client = self._get_client()
        audio_file = self._make_audio_file(audio_bytes, mime_type)
        effective_prompt = prompt if prompt and self._model in _PROMPT_SUPPORTED_MODELS else ""

        if self._model in _STREAMING_MODELS:
            return self._transcribe_streaming(client, audio_file, effective_prompt)
        return self._transcribe_blocking(client, audio_file, effective_prompt)

    def transcribe_file(
        self,
        path: str | Path,
        mime_type: str,
        prompt: str = "",
        on_progress: Callable[[int, int], None] | None = None,
    ) -> str:
        audio_path = Path(path)
        if not audio_path.exists() or audio_path.stat().st_size == 0:
            raise RuntimeError("No audio bytes provided")

        if mime_type.startswith("audio/pcm"):
            sample_rate = self._sample_rate_from_mime(mime_type)
            return self._transcribe_pcm_file(
                audio_path,
                sample_rate,
                prompt,
                on_progress,
            )

        size = audio_path.stat().st_size
        if mime_type in {"audio/wav", "audio/x-wav"} and size > _MAX_AUDIO_UPLOAD_BYTES:
            return self._transcribe_wav_file(audio_path, prompt, on_progress)
        if size > _MAX_AUDIO_UPLOAD_BYTES:
            raise RuntimeError(
                "This compressed audio file is larger than 24 MB. "
                "Convert it to a smaller MP3/M4A or split it before importing."
            )
        return self.transcribe_bytes(audio_path.read_bytes(), mime_type, prompt)

    def _transcribe_wav_file(
        self,
        path: Path,
        prompt: str,
        on_progress: Callable[[int, int], None] | None,
    ) -> str:
        with wave.open(str(path), "rb") as source:
            channels = source.getnchannels()
            sample_width = source.getsampwidth()
            sample_rate = source.getframerate()
            total_frames = source.getnframes()
            frame_bytes = channels * sample_width
            if channels <= 0 or sample_width <= 0 or sample_rate <= 0:
                raise RuntimeError("The WAV file has an invalid audio format")
            frames_per_chunk = max(1, (_MAX_AUDIO_UPLOAD_BYTES - 128) // frame_bytes)
            total_chunks = max(1, (total_frames + frames_per_chunk - 1) // frames_per_chunk)
            transcripts: list[str] = []
            for chunk_index in range(total_chunks):
                frames = source.readframes(frames_per_chunk)
                if not frames:
                    break
                if on_progress:
                    on_progress(chunk_index + 1, total_chunks)
                contextual_prompt = prompt
                if transcripts:
                    contextual_prompt = (
                        f"{prompt}\nPrevious segment transcript: {transcripts[-1][-1200:]}"
                    ).strip()
                wav = self._audio_frames_to_wav(
                    frames,
                    channels,
                    sample_width,
                    sample_rate,
                )
                transcripts.append(
                    self.transcribe_bytes(wav, "audio/wav", contextual_prompt).strip()
                )
        return " ".join(part for part in transcripts if part).strip()

    def _transcribe_pcm_file(
        self,
        path: Path,
        sample_rate: int,
        prompt: str,
        on_progress: Callable[[int, int], None] | None,
    ) -> str:
        bytes_per_frame = 2
        max_pcm_bytes = _MAX_AUDIO_UPLOAD_BYTES - 44
        max_pcm_bytes -= max_pcm_bytes % bytes_per_frame
        total_bytes = path.stat().st_size
        total_chunks = max(1, (total_bytes + max_pcm_bytes - 1) // max_pcm_bytes)
        transcripts: list[str] = []

        with path.open("rb") as source:
            for chunk_index in range(total_chunks):
                pcm = source.read(max_pcm_bytes)
                if not pcm:
                    break
                if on_progress:
                    on_progress(chunk_index + 1, total_chunks)
                contextual_prompt = prompt
                if transcripts:
                    previous_tail = transcripts[-1][-1200:]
                    contextual_prompt = (
                        f"{prompt}\nPrevious segment transcript: {previous_tail}"
                    ).strip()
                wav = self._pcm_to_wav(pcm, sample_rate)
                transcripts.append(
                    self.transcribe_bytes(wav, "audio/wav", contextual_prompt).strip()
                )

        return " ".join(part for part in transcripts if part).strip()

    @staticmethod
    def _pcm_to_wav(pcm: bytes, sample_rate: int) -> bytes:
        return OpenWhisperOpenAI._audio_frames_to_wav(pcm, 1, 2, sample_rate)

    @staticmethod
    def _audio_frames_to_wav(
        frames: bytes,
        channels: int,
        sample_width: int,
        sample_rate: int,
    ) -> bytes:
        output = io.BytesIO()
        with wave.open(output, "wb") as wav:
            wav.setnchannels(channels)
            wav.setsampwidth(sample_width)
            wav.setframerate(sample_rate)
            wav.writeframes(frames)
        return output.getvalue()

    @staticmethod
    def _sample_rate_from_mime(mime_type: str) -> int:
        match = re.search(r"(?:rate|samplerate)=(\d+)", mime_type, re.IGNORECASE)
        if not match:
            raise RuntimeError("PCM audio is missing its sample rate")
        sample_rate = int(match.group(1))
        if sample_rate <= 0:
            raise RuntimeError("PCM audio has an invalid sample rate")
        return sample_rate

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
            "audio/x-wav": ".wav",
            "audio/mpeg": ".mp3",
            "audio/mp3": ".mp3",
            "audio/mp4": ".m4a",
            "audio/x-m4a": ".m4a",
            "audio/ogg": ".ogg",
        }
        return mime_map.get(mime_type, ".webm")
