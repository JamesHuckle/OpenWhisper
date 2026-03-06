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

