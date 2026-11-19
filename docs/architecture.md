# WhisperFlow Architecture and Build Plan

## System overview

WhisperFlow is split into two runtime layers, in one process:

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
   - saved files: `settings.json` and each recording's `metadata.json` are
     saved through `src-tauri/src/json_file.rs`. It writes a `.tmp` file,
     flushes it to disk and renames it over the old file, keeping the old
     one as `.bak`; a damaged file is read from that backup instead, so a
     crash while saving never hides a recording or resets the settings. The
     Android app saves recording details with `AtomicFile`, under one lock
     shared by every `RecordingStore` of the same folder. It keeps the newest
     ten recordings, plus any older one still being captured or transcribed
     (`RecordingStore.reserve`), which goes at the next cleanup once done.
   - debug log: `src-tauri/src/debug_log.rs` writes each line to stderr and
     to `openwhisper.log` in the app log folder (release builds on Windows
     have no console). Past 1 MB the file becomes `openwhisper.log.1`,
     replacing the older one. The menu's "Open log folder" button opens the
     folder. Lines never hold dictated text, keys or prompts.
   - code layout: `src-tauri/src/lib.rs` holds the Tauri commands, the tray
     and start-up. Logic that needs no Tauri sits in files beside it
     (`settings.rs`, `recordings.rs`, `overlay_geometry.rs` and the files
     named above), and `src-tauri/core-tests` runs their tests on any system.
     The Windows-only `text_inserter.rs` and `overlay_positioner.rs` make the
     Win32 calls.
   - transcription: the app calls OpenAI itself, as the Android app does;
     nothing runs beside it. `transcription_sessions.rs` holds each
     dictation's session, `transcriber.rs` sends a saved recording (in
     parts when it is long) and tidies the text, `openai_api.rs` makes the
     two HTTP calls (reqwest with rustls; each is tried again after 1 s and
     3 s when the connection drops or the service answers 429, 500, 502, 503
     or 504, and a transcription's time limit grows with its upload) and
     `realtime.rs` is the Live websocket. The shared rules are in `audio_rules.rs` and
     `saved_parts.rs`. All of them need no Tauri, so `core-tests` runs their
     tests against local stand-ins for OpenAI.

## Data flow

1. User starts dictation from UI or global hotkey.
   The microphone opens first, in an audio context built once and kept
   suspended between recordings. The paste target is locked and the WAV
   file opened meanwhile; audio captured before the file opens is held,
   then saved first. The recording file commands run off the main thread.
2. The shell starts a session (`worker_start_session`) with the settings
   saved at that moment. A Live session opens its Realtime websocket at
   once. If it cannot, the page starts the session again with
   `standardOnly: true` (no websocket), keeps recording, and transcribes the
   saved recording with Standard when the user stops. Imports and retries
   always start their session with `standardOnly: true`, so they work while
   Live is selected.
3. The shell saves every audio chunk once, to the recording's WAV file
   (`src-tauri/src/recording_store.rs`). The page sends each chunk as raw
   bytes, with its recording or session id in a header, never as base64
   text. The file stays open while recording; its header is brought up to
   date about once a second. Its `metadata.json` is written only when the
   recording starts and closes: meanwhile the recordings list reads its size
   from the WAV file. At start-up, before the window can act, a recording an
   earlier run left "recording" (its header repaired) or "transcribing" is
   marked failed with a message to press Transcribe, as Android does
   (`recover_interrupted` in `src-tauri/src/recordings.rs`).
   Live transcription also sends each chunk to its Realtime websocket
   (`worker_append_audio_chunk`), because it transcribes while the user
   speaks. No copy of these chunks is kept: the session only counts their
   bytes and loudest sample, to tell silence or a quick tap from speech at
   the end. Only Live sessions accept chunks. A Live session moves to a new
   connection every 25 minutes of audio and joins the text.
4. The session pushes transcript updates and the final text as events.
5. The shell finalizes the transcription. A Live session commits its audio
   and waits for the text (`worker_finalize_session_audio`); a connection
   that closes without it is an error. When a Live dictation fails (its
   audio could not be sent, finalizing fails, or its session reports an
   error other than "No audio found"), the page stops it and transcribes the
   saved recording once with Standard, in a new session; that final text is
   pasted over the Live draft. A Live session handed its saved file is also
   transcribed with the Standard model. For standard transcription the shell
   closes the WAV file and the transcriber reads it from disk in parts
   (`recording_transcribe`). For a microphone recording the transcriber
   first encodes the WAV once as lossless FLAC (`src-tauri/src/flac_upload.rs`,
   about half the bytes for speech), split into parts that each fit one
   upload, with the byte range of the WAV each holds. The log records how
   long it took (`encode_ms`, about 60 ms per minute of audio in an ignored
   timing test in that file) and the WAV and FLAC sizes. The transcriber
   checks the WAV for silence and uploads the parts,
   one request each; a long recording's later parts are prompted with the
   text before them. When the service refuses a FLAC part the rest of the
   recording goes as WAV parts. The transcriber deletes the FLAC parts when
   it returns, fails or is cancelled. A recording sent in parts has each finished part saved
   in `parts.json` beside it (`parts_path`), so a retry after a failure sends
   only the parts that did not finish (the same file and key the Python
   worker used, so a retry begun before still sends only the rest). A silent part (the same silence level
   as the whole-recording check) is never uploaded and is saved as finished
   and empty, and a part whose text only repeats its prompt is kept with no
   text, as on Android. The shell deletes that file once the transcription completes
   or the recording is pruned. Android saves its parts the same way
   (`SavedParts.kt`). Retrying a saved
   recording sends only its id
   the same way, and an imported file is copied on disk after the native
   file picker returns its path, so audio never passes through the UI.
6. Native insertion engine writes the text into the field that was focused when
   the recording started, after checking that field again.

One-way data flow rule:
- UI and tree components never mutate parent-owned commit state during initialization.
- Pending state remains local until explicit save/commit action.

## Session commands and events

The window calls five commands; they kept the names they had when a Python
worker did the work:

- `worker_start_session { profileId }` returns `{ session_id }`. An
  unsupported model is refused; an empty saved key falls back to the
  `OPENAI_API_KEY` environment variable.
- `worker_append_audio_chunk` (raw bytes, `session-id` header) sends Live
  audio.
- `worker_finalize_session_audio { sessionId, mimeType }` finishes a Live
  session; `recording_transcribe { recordingId, sessionId }` finishes a
  Standard one from its saved recording.
- `worker_stop_session { sessionId }` cancels the session's token: a running
  upload or tidy request is dropped at once, and the session sends no more
  events.

Only the start returns a value. The others return nothing when they succeed,
and errors as `code: message` text, such as
`bad_request: Unknown session_id`.

Progress, live text, warnings and the result reach the window as the
`worker-session-event` Tauri event. `done` marks the session's last event (a
`final` or `error`), after which the session is forgotten:

```json
{ "session_id": "...", "event": { "type": "partial", "text": "Transcribing part 2 of 5..." }, "done": false }
```

Errors and warnings never quote the request: a failed transcription's
message has the vocabulary prompt removed, and a failed tidy pass reports
only the error type, status and code.

## Windows insertion engine

What the code in `apps/desktop/src-tauri/src/text_inserter.rs` does,
with the write rules in `paste_target.rs`:

1. Windows focus-change events (`SetWinEventHook`) track the last focused
   window outside OpenWhisper, so that clicking the pill still knows where
   the user was. Nothing polls: the keyboard hook wakes a dispatcher thread
   for Ctrl+Space, the settings menu is sent the program's name only when it
   changes, and the mouse hook wakes the pill's monitor check only when the
   cursor enters another monitor. That check then runs every 100 ms until
   the pill has moved, and otherwise once every 5 s for display changes.
   The pill moves while hidden and is shown once it has settled; if a move
   fails, a visible pill is shown again where it was and the move is retried
   after 1 s. A pill hidden on purpose stays hidden.
2. When a recording starts, `capture_paste_target` locks that window and its
   focused control for the whole recording. Focus tracking never changes it.
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
   held Ctrl is released first. Once a check fails, or Windows refuses a key,
   live typing stops for the rest of the recording.
5. The final text (`insert_final_text`) replaces the live draft with Backspace
   only when every check passed and live typing never stopped. Otherwise it
   is pasted after the draft and nothing is deleted. If the window is gone,
   the user moved to another app, or the field is a password field, the
   transcript is put on the clipboard instead and the user is told why.
6. The final paste saves the clipboard, sets the text marked to stay out of
   clipboard history, cloud sync and clipboard monitors, only then deletes
   the draft, presses Ctrl+V, waits for the target to handle it, then
   restores the saved clipboard unless something else changed it meanwhile.
   If Windows refuses the keys, the transcript stays on the clipboard. Bitmaps are restored in their
   DIB form; GDI-handle and private formats are not restored.
7. The transcript is saved with its recording before any of this. If copying
   or pasting then fails, the user is told it was saved and can copy it from
   the recording in the menu; the recording is never marked failed, so it is
   not transcribed (and paid for) again.

Not built yet: writing through UI Automation `ValuePattern` / `TextPattern`
and app-specific adapters.

## Build milestones

### Milestone 1: Core MVP
- Tauri shell with session controls
- session commands from the window to the native transcriber
- OpenAI model listing and test transcription endpoint

### Milestone 2: Streaming
- session event push stream (Tauri events)
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

### Desktop

Environment variables, read by the app when a dictation names no key or
model (no key saved in the settings, as in some development runs):
- `OPENAI_API_KEY`: used when no key is saved
- `OPENWHISPER_MODEL`: optional override (default: `gpt-transcribe`)
- `OPENWHISPER_POLISH_MODEL`, `OPENWHISPER_POLISH_REASONING`: optional
  tidy model and reasoning effort (default `gpt-5-nano`, `minimal`)

Use your package manager (`npm`, `pnpm`, or `bun`) and run Tauri dev mode.
On Linux (including WSL), saving the API key in the app needs a running
Secret Service such as GNOME Keyring (`gnome-keyring-daemon`). Without one,
the app reports that it could not save the key securely.

## Definition of done for production

- Dictation works in Chrome text fields, Electron apps, and common native editors.
- p95 final transcript latency under 1.2s.
- Insertion success over 95% on support matrix.
- Global hotkey and focus switching remain stable for long sessions.

