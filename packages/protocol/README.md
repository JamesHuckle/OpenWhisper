# OpenWhisper IPC Protocol

This package documents the canonical local IPC contract between desktop and worker.

## Shared model list

`models.json` is the one list of transcription models the app offers: the
default and live models, the menu label and hint for each, the models that
older saved settings move to, and the refine (tidy) model. The desktop settings
menu (`apps/desktop/src/models.ts`), the desktop's saved-settings checks
(`apps/desktop/src-tauri/src/models.rs`) and the worker
(`apps/worker/src/openwhisper_worker/models.py`) all read it. Change a model
here only; the packaged worker carries a copy that the Windows build adds.
The Android app talks to OpenAI directly and still names `gpt-transcribe` in
its own code.

## Example messages

`examples/` holds real request, reply and event messages. The worker's tests
check that it answers each request with the reply shown, and the desktop's
Rust tests check that it sends and reads the same messages. See
[`examples/README.md`](examples/README.md).

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
    "models": ["gpt-transcribe", "gpt-live-transcribe"]
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

## Sessions

Each dictation starts a session that carries every setting the worker uses.
Saving settings in the desktop therefore never restarts the worker. An empty
`openai_api_key` or `transcription_model` falls back to the worker's
`OPENAI_API_KEY` or `OPENWHISPER_MODEL`.

```json
{
  "id": "uuid",
  "method": "start_session",
  "params": {
    "profile_id": "default",
    "openai_api_key": "sk-...",
    "transcription_model": "gpt-transcribe",
    "prompt": "Glossary: ...",
    "refine_enabled": true,
    "refine_prompt": ""
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

A session can instead be finalized from a saved file on disk, with no chunks
appended. The worker reads the file in parts and never changes or removes it.
Live (realtime) sessions only accept appended chunks.

```json
{
  "id": "uuid",
  "method": "finalize_session_audio",
  "params": { "session_id": "uuid", "mime_type": "audio/wav", "audio_path": "C:\\...\\audio.wav" }
}
```
