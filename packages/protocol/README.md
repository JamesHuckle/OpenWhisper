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

## Dictation rules

`audio-rules.json` holds the rules every app follows before and after it
sends a recording to OpenAI. The worker (`apps/worker/src/openwhisper_worker/audio_rules.py`)
reads it at run time; the Android build (`apps/android/app/build.gradle.kts`)
turns it into `BuildConfig` values for `AudioRules`.

- `min_speech_seconds`: a recording shorter than this is an accidental tap.
  It is not sent, and the user sees `no_audio_message`.
- `silent_pcm_peak`: 16-bit audio whose loudest sample stays below this is a
  muted microphone. It is not sent either.
- `prompt_echo_min_share`: the models answer near-silent audio by repeating
  the prompt they were given. A transcript whose words (lower case, letters
  and digits only) appear in that order inside the prompt's words, and that
  is at least this share of the prompt's length, is treated as no speech.
  The desktop compares the whole transcript with the user's vocabulary
  prompt; Android, which sends only the previous part's text as a prompt,
  compares each part with that.
- `max_upload_bytes`: the largest file sent in one request. Longer PCM and
  WAV recordings are split into parts this size or smaller.
- `no_audio_message`: what the user sees for any of the cases above, or for
  an empty transcript.

`audio-examples/` holds WAV files and transcripts with the expected answer
for each rule; the worker's and Android's tests both run them. See
[`audio-examples/README.md`](audio-examples/README.md). The packaged worker
carries a copy of `audio-rules.json` that the Windows build adds.

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

A Live (realtime) session takes 16-bit PCM chunks while the user speaks and
is finalized without a file. The worker keeps no copy of the chunks; it only
counts their bytes and loudest sample to report silence or a quick tap as no
audio. Other sessions refuse chunks with `bad_request`.

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
  "params": { "session_id": "uuid", "mime_type": "audio/pcm;rate=24000" }
}
```

A Standard session is finalized from a saved file on disk instead. The worker
reads the file in parts and never changes or removes it. Live sessions refuse
a saved file.

```json
{
  "id": "uuid",
  "method": "finalize_session_audio",
  "params": { "session_id": "uuid", "mime_type": "audio/wav", "audio_path": "C:\\...\\audio.wav" }
}
```
