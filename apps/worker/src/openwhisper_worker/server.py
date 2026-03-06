from __future__ import annotations

import base64
import sys
import uuid
from dataclasses import dataclass
from typing import Any

from openwhisper_worker.openai_client import OpenWhisperOpenAI
from openwhisper_worker.protocol import WorkerError, WorkerRequest, WorkerResponse


@dataclass
class Session:
    session_id: str
    profile_id: str
    audio_bytes: bytearray
    events: list[dict[str, Any]]
    cursor: int = 0
    finalized: bool = False


class WorkerServer:
    def __init__(self) -> None:
        self._openai = OpenWhisperOpenAI()
        self._sessions: dict[str, Session] = {}

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
            if request.method == "poll_session_events":
                return self._handle_poll_session_events(request)
            return self._err(request.id, "not_found", f"Unknown method: {request.method}")
        except Exception as exc:  # noqa: BLE001
            return self._err(request.id, "internal_error", str(exc))

    def _handle_start_session(self, request: WorkerRequest) -> WorkerResponse:
        profile_id = str(request.params.get("profile_id", "default"))
        session_id = str(uuid.uuid4())
        self._sessions[session_id] = Session(
            session_id=session_id,
            profile_id=profile_id,
            audio_bytes=bytearray(),
            events=[{"type": "partial", "text": "Listening..."}],
        )
        return self._ok(request.id, {"session_id": session_id, "profile_id": profile_id})

    def _handle_stop_session(self, request: WorkerRequest) -> WorkerResponse:
        session_id = str(request.params.get("session_id", ""))
        if not session_id or session_id not in self._sessions:
            return self._err(request.id, "bad_request", "Unknown session_id")
        session = self._sessions.pop(session_id)
        return self._ok(
            request.id,
            {
                "session_id": session.session_id,
                "profile_id": session.profile_id,
                "final_text": "",
                "note": "Session stopped.",
            },
        )

    def _handle_poll_session_events(self, request: WorkerRequest) -> WorkerResponse:
        session_id = str(request.params.get("session_id", ""))
        if not session_id or session_id not in self._sessions:
            return self._err(request.id, "bad_request", "Unknown session_id")

        session = self._sessions[session_id]
        if session.cursor >= len(session.events):
            return self._ok(request.id, {"events": []})

        event = session.events[session.cursor]
        session.cursor += 1
        done = session.finalized and session.cursor >= len(session.events)
        return self._ok(
            request.id,
            {
                "events": [event],
                "done": done,
            },
        )

    def _handle_append_audio_chunk(self, request: WorkerRequest) -> WorkerResponse:
        session_id = str(request.params.get("session_id", ""))
        chunk_b64 = str(request.params.get("chunk_base64", ""))
        if not session_id or session_id not in self._sessions:
            return self._err(request.id, "bad_request", "Unknown session_id")
        if not chunk_b64:
            return self._err(request.id, "bad_request", "chunk_base64 is required")

        try:
            chunk = base64.b64decode(chunk_b64)
        except Exception as exc:  # noqa: BLE001
            return self._err(request.id, "bad_request", f"Invalid base64 chunk: {exc}")

        session = self._sessions[session_id]
        session.audio_bytes.extend(chunk)
        kb = len(session.audio_bytes) // 1024
        session.events.append({"type": "partial", "text": f"Captured {kb} KB of audio..."})
        return self._ok(request.id, {"buffered_bytes": len(session.audio_bytes)})

    def _handle_finalize_session_audio(self, request: WorkerRequest) -> WorkerResponse:
        session_id = str(request.params.get("session_id", ""))
        mime_type = str(request.params.get("mime_type", "audio/webm"))
        if not session_id or session_id not in self._sessions:
            return self._err(request.id, "bad_request", "Unknown session_id")

        session = self._sessions[session_id]
        if not session.audio_bytes:
            return self._err(request.id, "bad_request", "No audio captured for session")

        text = self._openai.transcribe_bytes(bytes(session.audio_bytes), mime_type=mime_type)
        session.events.append({"type": "final", "text": text})
        session.finalized = True
        return self._ok(request.id, {"final_text": text})

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


def run_stdio_server() -> None:
    server = WorkerServer()
    while True:
        line = sys.stdin.readline()
        if line == "":
            break
        line = line.strip()
        if not line:
            continue

        try:
            req = WorkerRequest.model_validate_json(line)
            res = server.handle(req)
        except Exception as exc:  # noqa: BLE001
            res = WorkerResponse(
                id="",
                ok=False,
                error=WorkerError(code="bad_request", message=str(exc)),
            )

        payload = res.model_dump_json()
        sys.stdout.write(payload + "\n")
        sys.stdout.flush()

