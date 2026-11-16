from __future__ import annotations

import base64
import os
import re
import sys
import tempfile
import threading
import uuid
import wave
from array import array
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from typing import IO, Any, Callable

from openwhisper_worker.audio_rules import (
    MIN_SPEECH_SECONDS,
    NO_AUDIO_MESSAGE,
    PROMPT_ECHO_MIN_SHARE,
    SILENT_PCM_PEAK,
)
from openwhisper_worker.openai_client import (
    OpenWhisperOpenAI,
    RealtimeTranscriptionStream,
    describe_error,
    is_short_clean_transcript,
)
from openwhisper_worker.protocol import (
    SessionEvent,
    WorkerError,
    WorkerRequest,
    WorkerResponse,
)


@dataclass
class Session:
    session_id: str
    profile_id: str
    prompt: str
    refine_enabled: bool
    refine_prompt: str
    # The OpenAI client for the key and model this session started with. A
    # later dictation with other settings never changes it.
    openai: OpenWhisperOpenAI
    # The worker's own copy of appended chunks, created by the first chunk.
    # A session finalized from a saved file never appends and never has one.
    audio_path: str = ""
    audio_bytes: int = 0
    realtime_stream: RealtimeTranscriptionStream | None = None
    realtime_bytes: int = 0
    realtime_segments: list[str] | None = None
    finalizing: bool = False
    # Set by stop_session. A running transcription checks it between audio
    # parts and before tidying, and then stops without a result.
    cancelled: threading.Event = field(default_factory=threading.Event)
    # Guards finalizing and cancelled against stop_session on another thread.
    lock: threading.Lock = field(default_factory=threading.Lock)


class _Cancelled(Exception):
    """The session was stopped while it was being transcribed."""


_REALTIME_ROTATE_BYTES = 24_000 * 2 * 25 * 60
# Requests run side by side so a long transcription never holds up a cancel,
# a ping or the next session. One session's requests still arrive one by one.
_MAX_PARALLEL_REQUESTS = 4
_PCM_SCAN_BYTES = 1024 * 1024
# Shorter prompt fragments are too common to remove from an error message
# without mangling it.
_MIN_REDACT_CHARS = 4


def _without_private_text(message: str, *private: str) -> str:
    """Remove the user's prompts from an error message before it leaves the worker.

    Errors can quote the request they came from; the prompts are the user's own
    words and must not be copied into notices, logs or bug reports.
    """
    for value in sorted({p.strip() for p in private}, key=len, reverse=True):
        if len(value) >= _MIN_REDACT_CHARS:
            message = message.replace(value, "[redacted]")
    return message


def _normalize_words(text: str) -> str:
    return " ".join(re.findall(r"[a-z0-9]+", text.lower()))


def _echoes_prompt(text: str, prompt: str) -> bool:
    """Transcription models answer silent or near-silent audio by repeating
    the vocabulary prompt. Such a transcript is no speech, not dictation."""
    spoken = _normalize_words(text)
    hints = _normalize_words(prompt)
    if not spoken or not hints:
        return False
    return spoken in hints and len(spoken) >= len(hints) * PROMPT_ECHO_MIN_SHARE


def _pcm_is_silent(read: Callable[[int], bytes]) -> bool:
    """True when 16-bit samples from `read` never rise above near-silence."""
    peak = 0
    while True:
        block = read(_PCM_SCAN_BYTES)
        if not block:
            return True
        usable = len(block) - (len(block) % 2)
        if usable:
            samples = array("h")
            samples.frombytes(block[:usable])
            if sys.byteorder == "big":
                samples.byteswap()
            peak = max(peak, max(samples), -min(samples))
            if peak >= SILENT_PCM_PEAK:
                return False


def _pcm_rate(mime_type: str) -> int | None:
    if not mime_type.startswith("audio/pcm"):
        return None
    match = re.search(r"(?:rate|samplerate)=(\d+)", mime_type, re.IGNORECASE)
    return int(match.group(1)) if match else None


def _no_speech_reason(path: str, mime_type: str, size: int) -> str | None:
    """Why 16-bit PCM audio (raw or WAV) holds no speech, or None.

    Raw PCM is what the worker collects from appended chunks; WAV is how the
    desktop saves a microphone recording. Other audio is left to the model.
    """
    rate = _pcm_rate(mime_type)
    if rate:
        seconds = size / (rate * 2)
        if seconds < MIN_SPEECH_SECONDS:
            return f"too_short={seconds:.2f}s"
        with open(path, "rb") as source:
            return "silent" if _pcm_is_silent(source.read) else None
    if mime_type not in {"audio/wav", "audio/x-wav"}:
        return None
    try:
        with wave.open(path, "rb") as source:
            frame_bytes = source.getnchannels() * source.getsampwidth()
            if source.getsampwidth() != 2 or source.getframerate() <= 0:
                return None
            seconds = source.getnframes() / source.getframerate()
            if seconds < MIN_SPEECH_SECONDS:
                return f"too_short={seconds:.2f}s"
            silent = _pcm_is_silent(lambda n: source.readframes(max(1, n // frame_bytes)))
            return "silent" if silent else None
    except (wave.Error, EOFError, OSError):
        # Not a plain PCM WAV; the model decides what it holds.
        return None


EventSink = Callable[[SessionEvent], None]
OpenAIFactory = Callable[[str, str], OpenWhisperOpenAI]
_TERMINAL_EVENTS = {"final", "error"}


class WorkerServer:
    def __init__(
        self,
        emit: EventSink | None = None,
        openai_factory: OpenAIFactory = OpenWhisperOpenAI,
    ) -> None:
        # Used by ping and list_models, and by a session that names no key or
        # model (the worker run on its own, with its environment variables).
        self._openai = OpenWhisperOpenAI()
        self._openai_factory = openai_factory
        # The client for the key and model the last dictation named. It is
        # kept while they stay the same, so its connection stays warm, and
        # replaced when the user changes them; the worker never restarts.
        self._configured: tuple[tuple[str, str], OpenWhisperOpenAI] | None = None
        self._configured_lock = threading.Lock()
        self._sessions: dict[str, Session] = {}
        self._sessions_lock = threading.Lock()
        self._emit_sink: EventSink = emit or (lambda _event: None)

    def handle(self, request: WorkerRequest) -> WorkerResponse:
        try:
            if request.method == "ping":
                return self._ok(request.id, self._openai.health())
            if request.method == "list_models":
                return self._ok(request.id, {"models": self._openai.available_models()})
            if request.method == "start_session":
                return self._handle_start_session(request)
            if request.method == "stop_session":
                return self._handle_stop_session(request)
            if request.method == "append_audio_chunk":
                return self._handle_append_audio_chunk(request)
            if request.method == "finalize_session_audio":
                return self._handle_finalize_session_audio(request)
            return self._err(request.id, "not_found", f"Unknown method: {request.method}")
        except Exception as exc:  # noqa: BLE001
            return self._err(request.id, "internal_error", str(exc))

    def close(self) -> None:
        """Stop every open session; running transcriptions end at their next check."""
        with self._sessions_lock:
            sessions = list(self._sessions.values())
            self._sessions.clear()
        for session in sessions:
            self._cancel(session)

    def _openai_for(self, params: dict[str, Any]) -> OpenWhisperOpenAI:
        """The client for the key and model a start_session request names."""
        api_key = str(params.get("openai_api_key") or "").strip()
        model = str(params.get("transcription_model") or "").strip()
        if not api_key and not model:
            return self._openai
        settings = (api_key, model)
        with self._configured_lock:
            if self._configured is None or self._configured[0] != settings:
                self._configured = (settings, self._openai_factory(api_key, model))
            return self._configured[1]

    def _session(self, request: WorkerRequest) -> Session | None:
        session_id = str(request.params.get("session_id", ""))
        with self._sessions_lock:
            return self._sessions.get(session_id) if session_id else None

    def _emit(self, session: Session, event: dict[str, Any]) -> None:
        """Send one session update to the desktop at once.

        A final or error event ends the session, so it is forgotten here and
        its transcript does not stay in memory.
        """
        if session.cancelled.is_set():
            return
        done = event.get("type") in _TERMINAL_EVENTS
        if done:
            with self._sessions_lock:
                self._sessions.pop(session.session_id, None)
        self._emit_sink(SessionEvent(session_id=session.session_id, event=event, done=done))

    def _handle_start_session(self, request: WorkerRequest) -> WorkerResponse:
        profile_id = str(request.params.get("profile_id", "default"))
        prompt = str(request.params.get("prompt", ""))
        refine_enabled = bool(request.params.get("refine_enabled", True))
        refine_prompt = str(request.params.get("refine_prompt", ""))
        try:
            openai = self._openai_for(request.params)
        except ValueError as exc:
            return self._err(request.id, "bad_request", str(exc))
        # Never log the key, the prompts or the dictated text.
        sys.stderr.write(
            "[worker] start_session "
            f"profile_id={profile_id!r} "
            f"model={openai.model} "
            f"transcription_prompt_chars={len(prompt)} "
            f"refine_enabled={refine_enabled} "
            f"refine_prompt_chars={len(refine_prompt)}\n"
        )
        sys.stderr.flush()
        session_id = str(uuid.uuid4())
        session = Session(
            session_id=session_id,
            profile_id=profile_id,
            prompt=prompt,
            refine_enabled=refine_enabled,
            refine_prompt=refine_prompt,
            openai=openai,
            realtime_segments=[],
        )
        if openai.is_live:
            session.realtime_stream = self._start_realtime(session)
        with self._sessions_lock:
            self._sessions[session_id] = session
        return self._ok(request.id, {"session_id": session_id, "profile_id": profile_id})

    def _start_realtime(self, session: Session) -> RealtimeTranscriptionStream:
        return session.openai.start_realtime_transcription(
            lambda delta: self._emit(session, {"type": "partial", "text": delta})
        )

    def _handle_stop_session(self, request: WorkerRequest) -> WorkerResponse:
        session_id = str(request.params.get("session_id", ""))
        with self._sessions_lock:
            session = self._sessions.pop(session_id, None) if session_id else None
        if session is None:
            return self._err(request.id, "bad_request", "Unknown session_id")
        self._cancel(session)
        return self._ok(
            request.id,
            {
                "session_id": session.session_id,
                "profile_id": session.profile_id,
                "final_text": "",
                "note": "Session stopped.",
            },
        )

    def _cancel(self, session: Session) -> None:
        with session.lock:
            session.cancelled.set()
            finalizing = session.finalizing
            stream = session.realtime_stream
        if stream:
            stream.close()
        # A running transcription may still read the audio file; it removes
        # the file itself when it stops.
        if not finalizing:
            self._remove_audio_file(session)

    def _handle_append_audio_chunk(self, request: WorkerRequest) -> WorkerResponse:
        chunk_b64 = str(request.params.get("chunk_base64", ""))
        session = self._session(request)
        if session is None:
            return self._err(request.id, "bad_request", "Unknown session_id")
        if not chunk_b64:
            return self._err(request.id, "bad_request", "chunk_base64 is required")
        try:
            chunk = base64.b64decode(chunk_b64)
        except Exception as exc:  # noqa: BLE001
            return self._err(request.id, "bad_request", f"Invalid base64 chunk: {exc}")

        # Under the lock, so stop_session cannot remove the file (or miss a
        # new one) mid-write and finalizing never reads a half-written chunk.
        with session.lock:
            if session.cancelled.is_set():
                return self._err(request.id, "bad_request", "Unknown session_id")
            if session.finalizing:
                return self._err(request.id, "bad_request", "Session is already finalizing")
            if not session.audio_path:
                audio_file = tempfile.NamedTemporaryFile(
                    prefix=f"openwhisper-{session.session_id}-",
                    suffix=".audio",
                    delete=False,
                )
                audio_file.close()
                session.audio_path = audio_file.name
            with open(session.audio_path, "ab") as audio_file:
                audio_file.write(chunk)
            session.audio_bytes += len(chunk)
        if session.realtime_stream:
            if session.realtime_bytes and (
                session.realtime_bytes + len(chunk) > _REALTIME_ROTATE_BYTES
            ):
                segment = session.realtime_stream.finalize().strip()
                if segment:
                    session.realtime_segments.append(segment)
                if session.cancelled.is_set():
                    return self._err(request.id, "bad_request", "Unknown session_id")
                replacement = self._start_realtime(session)
                # stop_session reads the stream under the same lock, so it
                # either closes this replacement or we see it was cancelled.
                with session.lock:
                    cancelled = session.cancelled.is_set()
                    if not cancelled:
                        session.realtime_stream = replacement
                if cancelled:
                    replacement.close()
                    return self._err(request.id, "bad_request", "Unknown session_id")
                session.realtime_bytes = 0
            session.realtime_stream.append(chunk)
            session.realtime_bytes += len(chunk)
        else:
            kb = session.audio_bytes // 1024
            self._emit(session, {"type": "partial", "text": f"Captured {kb} KB of audio..."})
        return self._ok(request.id, {"buffered_bytes": session.audio_bytes})

    def _handle_finalize_session_audio(self, request: WorkerRequest) -> WorkerResponse:
        mime_type = str(request.params.get("mime_type", "audio/webm"))
        # A saved recording on disk, given in place of appended chunks. The
        # worker reads it in parts and never changes or removes it.
        audio_path = str(request.params.get("audio_path") or "")
        session = self._session(request)
        if session is None:
            return self._err(request.id, "bad_request", "Unknown session_id")
        with session.lock:
            if session.finalizing:
                return self._err(request.id, "bad_request", "Session is already finalizing")
            if session.cancelled.is_set():
                return self._cancelled(request.id, session)
            session.finalizing = True

        try:
            return self._finalize(request.id, session, mime_type, audio_path)
        except _Cancelled:
            return self._cancelled(request.id, session)
        finally:
            # Finalizing always ends a session, even when it fails.
            with self._sessions_lock:
                self._sessions.pop(session.session_id, None)
            self._remove_audio_file(session)

    def _check_cancelled(self, session: Session) -> None:
        if session.cancelled.is_set():
            raise _Cancelled()

    def _finalize(
        self, request_id: str, session: Session, mime_type: str, audio_path: str = ""
    ) -> WorkerResponse:
        session_id = session.session_id
        if audio_path and session.realtime_stream:
            # Live transcription hears audio as it is appended; it cannot
            # read a saved file.
            session.realtime_stream.close()
            session.realtime_stream = None
            self._emit(
                session,
                {
                    "type": "error",
                    "text": "Choose Standard transcription for saved audio files",
                },
            )
            return self._ok(request_id, {"final_text": ""})
        if audio_path:
            source_path = audio_path
            try:
                source_bytes = os.path.getsize(audio_path)
            except OSError:
                source_bytes = 0
        else:
            source_path = session.audio_path
            source_bytes = session.audio_bytes
        if source_bytes == 0:
            return self._finish_without_audio(request_id, session, "no_bytes")

        if not session.realtime_segments:
            reason = _no_speech_reason(source_path, mime_type, source_bytes)
            if reason:
                return self._finish_without_audio(request_id, session, reason)

        def on_progress(current: int, total: int) -> None:
            # Called before each audio part is uploaded: stop here on cancel.
            self._check_cancelled(session)
            self._emit(
                session,
                {"type": "partial", "text": f"Transcribing part {current} of {total}..."},
            )

        try:
            if session.realtime_stream:
                final_segment = session.realtime_stream.finalize().strip()
                session.realtime_stream = None
                text = " ".join(
                    [*(session.realtime_segments or []), final_segment]
                ).strip()
            else:
                text = session.openai.transcribe_file(
                    source_path,
                    mime_type=mime_type,
                    prompt=session.prompt,
                    on_progress=on_progress,
                )
        except _Cancelled:
            raise
        except Exception as exc:  # noqa: BLE001
            self._check_cancelled(session)
            msg = _without_private_text(str(exc), session.prompt).strip()
            self._emit(session, {"type": "error", "text": msg or exc.__class__.__name__})
            return self._ok(request_id, {"final_text": ""})

        self._check_cancelled(session)
        if not text.strip():
            return self._finish_without_audio(request_id, session, "empty_transcript")
        if _echoes_prompt(text, session.prompt):
            return self._finish_without_audio(request_id, session, "prompt_echo")

        # A short clean transcript is pasted at once: the default tidy pass
        # would not change it. A custom prompt can ask for any change, so it
        # always runs.
        skip_short_clean = (
            session.refine_enabled
            and not session.refine_prompt.strip()
            and is_short_clean_transcript(text)
        )
        if session.refine_enabled and not skip_short_clean:
            sys.stderr.write(
                "[worker] refine begin "
                f"session_id={session_id!r} "
                f"text_chars={len(text)} "
                f"custom_prompt_chars={len(session.refine_prompt)}\n"
            )
            sys.stderr.flush()
            self._emit(session, {"type": "partial", "text": "Refining transcript..."})
            try:
                text = session.openai.polish(text, session.refine_prompt)
            except Exception as exc:  # noqa: BLE001
                # The error text can quote the transcript or custom prompt that
                # was sent, so report only its type and codes.
                error = describe_error(exc)
                sys.stderr.write(
                    f"[worker] refine failed session_id={session_id!r} error={error}\n"
                )
                sys.stderr.flush()
                self._emit(
                    session,
                    {
                        "type": "warning",
                        "text": f"Refinement failed; using the raw transcript ({error})",
                    },
                )
                pass  # fall back to raw transcript on any LLM error
            else:
                sys.stderr.write(
                    f"[worker] refine done session_id={session_id!r} text_chars={len(text)}\n"
                )
                sys.stderr.flush()
            self._check_cancelled(session)
        else:
            reason = "short_clean" if skip_short_clean else "disabled"
            sys.stderr.write(
                f"[worker] refine skipped session_id={session_id!r} reason={reason}\n"
            )
            sys.stderr.flush()

        self._emit(session, {"type": "final", "text": text})
        return self._ok(request_id, {"final_text": text})

    def _cancelled(self, request_id: str, session: Session) -> WorkerResponse:
        sys.stderr.write(
            f"[worker] finalize_session_audio session_id={session.session_id!r} cancelled\n"
        )
        sys.stderr.flush()
        return self._ok(request_id, {"final_text": "", "cancelled": True})

    def _finish_without_audio(
        self, request_id: str, session: Session, reason: str
    ) -> WorkerResponse:
        sys.stderr.write(
            f"[worker] finalize_session_audio session_id={session.session_id!r} "
            f"no_audio={reason}\n"
        )
        sys.stderr.flush()
        if session.realtime_stream:
            session.realtime_stream.close()
            session.realtime_stream = None
        self._emit(session, {"type": "error", "text": NO_AUDIO_MESSAGE})
        return self._ok(request_id, {"final_text": ""})

    @staticmethod
    def _remove_audio_file(session: Session) -> None:
        if not session.audio_path:
            return
        try:
            os.remove(session.audio_path)
        except OSError:
            # Already gone, or (on Windows) still open elsewhere; stopping a
            # session must not fail because of it.
            pass

    @staticmethod
    def _ok(request_id: str, result: dict[str, Any]) -> WorkerResponse:
        return WorkerResponse(id=request_id, ok=True, result=result)

    @staticmethod
    def _err(request_id: str, code: str, message: str) -> WorkerResponse:
        return WorkerResponse(
            id=request_id,
            ok=False,
            error=WorkerError(code=code, message=message),
        )


class _LineWriter:
    """Writes whole JSON lines to stdout; responses and events come from many threads."""

    def __init__(self, stream: IO[str]) -> None:
        self._stream = stream
        self._lock = threading.Lock()

    def write(self, message: WorkerResponse | SessionEvent) -> None:
        payload = message.model_dump_json()
        with self._lock:
            self._stream.write(payload + "\n")
            self._stream.flush()


def _answer(server: WorkerServer, request: WorkerRequest, writer: _LineWriter) -> None:
    writer.write(server.handle(request))


def serve(
    requests: IO[str],
    responses: IO[str],
    server_factory: Callable[[EventSink], WorkerServer] = WorkerServer,
    max_parallel: int = _MAX_PARALLEL_REQUESTS,
) -> None:
    """Read requests on this thread and answer each one from a small pool.

    A reply is written when its request finishes, so replies can arrive in a
    different order than the requests; each carries its request id. Session
    events are written as they happen.
    """
    writer = _LineWriter(responses)
    server = server_factory(writer.write)
    pool = ThreadPoolExecutor(max_workers=max_parallel, thread_name_prefix="worker-request")
    try:
        while True:
            line = requests.readline()
            if line == "":
                break
            line = line.strip()
            if not line:
                continue
            try:
                req = WorkerRequest.model_validate_json(line)
            except Exception as exc:  # noqa: BLE001
                writer.write(
                    WorkerResponse(
                        id="",
                        ok=False,
                        error=WorkerError(code="bad_request", message=str(exc)),
                    )
                )
                continue
            pool.submit(_answer, server, req, writer)
    finally:
        # The desktop has gone: stop running transcriptions at their next check.
        # Requests already read still get their reply (a stopped session's
        # answer is quick), so piped input is never dropped.
        server.close()
        pool.shutdown(wait=True)


def run_stdio_server() -> None:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="strict", newline="\n", write_through=True)
    if hasattr(sys.stderr, "reconfigure"):
        sys.stderr.reconfigure(
            encoding="utf-8",
            errors="replace",
            newline="\n",
            write_through=True,
        )
    serve(sys.stdin, sys.stdout)
