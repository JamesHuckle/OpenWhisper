from __future__ import annotations

from typing import Any

from pydantic import BaseModel, Field


class WorkerRequest(BaseModel):
    id: str
    method: str
    params: dict[str, Any] = Field(default_factory=dict)


class WorkerError(BaseModel):
    code: str
    message: str


class WorkerResponse(BaseModel):
    id: str
    ok: bool
    result: dict[str, Any] | None = None
    error: WorkerError | None = None



class SessionEvent(BaseModel):
    """A session update the worker writes to stdout as soon as it happens.

    It shares stdout with responses; it has no `id`, so readers tell the two
    apart by the `session_id` field. `done` marks the session's last event.
    """

    session_id: str
    event: dict[str, Any]
    done: bool = False
