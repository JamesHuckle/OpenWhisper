# WhisperFlow IPC Protocol

This package documents the canonical local IPC contract between desktop and worker.

## Request

```json
{
  "id": "uuid",
  "method": "list_models",
  "params": {}
}
```

## Response

```json
{
  "id": "uuid",
  "ok": true,
  "result": {
    "models": ["gpt-4o-mini-transcribe"]
  }
}
```

## Error

```json
{
  "id": "uuid",
  "ok": false,
  "error": {
    "code": "bad_request",
    "message": "Unknown method"
  }
}
```

## Audio Commands

```json
{
  "id": "uuid",
  "method": "append_audio_chunk",
  "params": { "session_id": "uuid", "chunk_base64": "..." }
}
```

```json
{
  "id": "uuid",
  "method": "finalize_session_audio",
  "params": { "session_id": "uuid", "mime_type": "audio/webm" }
}
```
