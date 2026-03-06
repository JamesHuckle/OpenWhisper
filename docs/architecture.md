# WhisperFlow Architecture and Build Plan

## System overview

WhisperFlow is split into three runtime layers:

1. Desktop shell (`apps/desktop`, Tauri 2):
   - settings UI
   - transcription session controls
   - floating status UI and tray integration
2. Native control plane (`apps/desktop/src-tauri`, Rust):
   - global hotkeys
   - app focus and target detection
   - insertion engine (UI Automation, keyboard, clipboard fallback)
3. Model worker (`apps/worker`, Python):
   - OpenAI model catalog and routing
   - speech session orchestration
   - transcript post-processing

## Data flow

1. User starts dictation from UI or global hotkey.
2. Desktop shell opens session in worker via JSON-over-stdio protocol.
3. Audio chunks stream from shell/native capture to worker using `append_audio_chunk`.
4. Worker emits transcript updates and final text.
5. Desktop finalizes transcription using `finalize_session_audio`.
6. Native insertion engine commits text to focused target after explicit user action.

One-way data flow rule:
- UI and tree components never mutate parent-owned commit state during initialization.
- Pending state remains local until explicit save/commit action.

## IPC contract

Each request from desktop to worker:

```json
{ "id": "uuid", "method": "list_models", "params": {} }
```

Audio pipeline methods:

```json
{ "id": "uuid", "method": "append_audio_chunk", "params": { "session_id": "...", "chunk_base64": "..." } }
```

```json
{ "id": "uuid", "method": "finalize_session_audio", "params": { "session_id": "...", "mime_type": "audio/webm" } }
```

Each response from worker:

```json
{ "id": "uuid", "ok": true, "result": { "models": [] } }
```

Error response:

```json
{ "id": "uuid", "ok": false, "error": { "code": "bad_request", "message": "..." } }
```

## Windows insertion engine strategy

Ordered fallback:

1. UI Automation `ValuePattern` / `TextPattern`
2. App-specific adapters (browser extension bridge for complex editors)
3. `SendInput` keystroke injection
4. Transactional clipboard paste and restore

Safety rules:
- Never insert into password fields
- Re-validate focused window before commit
- Keep retry windows short and bounded

## Build milestones

### Milestone 1: Core MVP
- Tauri shell with session controls
- Python worker request/response protocol
- OpenAI model listing and test transcription endpoint

### Milestone 2: Streaming
- session event loop (polling in scaffold, push stream in production)
- partial/final transcript events in desktop UI
- realtime session transport to OpenAI speech endpoint

### Milestone 3: Native reliability
- hotkey registration
- focus target resolver
- insertion fallback chain

### Milestone 4: Product polish
- profiles per app
- rewrite/command mode
- updater, crash reporting, signed builds

## Local development

### Worker

Recommended tooling:
- `uv sync`
- `uv run python -m whisperflow_worker`

Environment variables:
- `OPENAI_API_KEY`: required for live transcription calls
- `WHISPERFLOW_MODEL`: optional override (default: `gpt-4o-mini-transcribe`)

### Desktop

Use your package manager (`npm`, `pnpm`, or `bun`) and run Tauri dev mode.

## Definition of done for production

- Dictation works in Chrome text fields, Electron apps, and common native editors.
- p95 final transcript latency under 1.2s.
- Insertion success over 95% on support matrix.
- Global hotkey and focus switching remain stable for long sessions.

