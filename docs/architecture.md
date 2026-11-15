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
   - insertion engine (Unicode keystrokes for live text, clipboard paste and
     restore for the final text, UI Automation password check)
   - settings: `settings.json` in the app config folder holds the ordinary
     settings; the OpenAI API key is kept in the OS credential store (Windows
     Credential Manager, macOS Keychain, Linux Secret Service), never in the
     file. On start, a plain key left by an older version is moved into the
     credential store and removed from the file only after the move succeeds
     (`src-tauri/src/api_key_store.rs`).
3. Model worker (`apps/worker`, Python):
   - OpenAI model catalog and routing
   - speech session orchestration
   - transcript post-processing

## Data flow

1. User starts dictation from UI or global hotkey.
2. Desktop shell opens session in worker via JSON-over-stdio protocol.
3. The shell saves every audio chunk once, to the recording's WAV file
   (`src-tauri/src/recording_store.rs`). The file stays open while recording;
   its header and size are brought up to date about once a second.
   Live transcription also streams each chunk to the worker using
   `append_audio_chunk`, because it transcribes while the user speaks.
4. Worker pushes transcript updates and final text as session events.
5. Desktop finalizes transcription using `finalize_session_audio`. For
   standard transcription the shell closes the WAV file and passes its
   `audio_path`, and the worker reads it from disk in parts
   (`recording_transcribe`). Retrying a saved recording sends only its id
   the same way, and an imported file is copied on disk after the native
   file picker returns its path, so audio never passes through the UI.
6. Native insertion engine writes the text into the field that was focused when
   the recording started, after checking that field again.

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

```json
{ "id": "uuid", "method": "finalize_session_audio", "params": { "session_id": "...", "mime_type": "audio/wav", "audio_path": "C:\\...\\audio.wav" } }
```

Each response from worker:

```json
{ "id": "uuid", "ok": true, "result": { "models": [] } }
```

Error response:

```json
{ "id": "uuid", "ok": false, "error": { "code": "bad_request", "message": "..." } }
```

Requests run side by side in the worker, so replies can arrive in a different
order than the requests; each reply carries its request's `id`. The desktop
waits for each reply within a time limit per method and restarts a worker that
stops. `stop_session` is answered at once even while that session is being
transcribed, and the transcription stops before its next audio part or the
tidy pass.

Session events share stdout with replies. They have no `id` and are written as
soon as they happen; the desktop forwards them to the UI as the
`worker-session-event` Tauri event. `done` marks the session's last event,
after which the worker forgets the session:

```json
{ "session_id": "...", "event": { "type": "partial", "text": "Transcribing part 2 of 5..." }, "done": false }
```

## Windows insertion engine

What the code in `apps/desktop/src-tauri/src/lib.rs` (`text_inserter`) does,
with the write rules in `paste_target.rs`:

1. A 20 ms loop tracks the last focused window outside OpenWhisper, so that
   clicking the pill still knows where the user was.
2. When a recording starts, `capture_paste_target` locks that window and its
   focused control for the whole recording. The loop never changes it.
3. Before every write the target is checked again:
   - the locked window still exists and has focus; it gets focus back only
     when the user merely clicked OpenWhisper, never from another app;
   - the focused control is the one locked at the start;
   - the focused field is not a password field (UI Automation `IsPassword`,
     plus `ES_PASSWORD` on classic edit controls);
   - the user has not pressed a key or clicked outside OpenWhisper since the
     last write (low-level keyboard and mouse hooks);
   - Alt and Windows are not held.
4. Live pieces (`type_to_target`) are typed as `SendInput` Unicode keystrokes,
   never through the clipboard. Line breaks and tabs are typed as spaces. A
   held Ctrl is released first. Once a check fails, live typing stops for the
   rest of the recording.
5. The final text (`insert_final_text`) replaces the live draft with Backspace
   only when every check passed and live typing never stopped. Otherwise it
   is pasted after the draft and nothing is deleted. If the window is gone,
   the user moved to another app, or the field is a password field, the
   transcript is put on the clipboard instead and the user is told why.
6. The final paste saves the clipboard, sets the text marked to stay out of
   clipboard history, cloud sync and clipboard monitors, presses Ctrl+V,
   waits for the target to handle it, then restores the saved clipboard
   unless something else changed it meanwhile. Bitmaps are restored in their
   DIB form; GDI-handle and private formats are not restored.

Not built yet: writing through UI Automation `ValuePattern` / `TextPattern`
and app-specific adapters.

## Build milestones

### Milestone 1: Core MVP
- Tauri shell with session controls
- Python worker request/response protocol
- OpenAI model listing and test transcription endpoint

### Milestone 2: Streaming
- session event push stream (worker stdout to Tauri events)
- partial/final transcript events in desktop UI
- realtime session transport to OpenAI speech endpoint

### Milestone 3: Native reliability
- hotkey registration
- focus target resolver
- insertion into more editors (UI Automation text patterns, app adapters)

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
- `WHISPERFLOW_MODEL`: optional override (default: `gpt-transcribe`)

### Desktop

Use your package manager (`npm`, `pnpm`, or `bun`) and run Tauri dev mode.
On Linux (including WSL), saving the API key in the app needs a running
Secret Service such as GNOME Keyring (`gnome-keyring-daemon`). Without one,
the app reports that it could not save the key securely.

## Definition of done for production

- Dictation works in Chrome text fields, Electron apps, and common native editors.
- p95 final transcript latency under 1.2s.
- Insertion success over 95% on support matrix.
- Global hotkey and focus switching remain stable for long sessions.

