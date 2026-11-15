# WhisperFlow Worker

Python worker process that receives JSON requests from desktop over stdin/stdout.
It answers requests side by side and writes session events (progress, live
text, the final transcript) to stdout as they happen; see the IPC contract in
`docs/architecture.md`.

## Run

```bash
uv sync
uv run python -m whisperflow_worker
```

The desktop app sends the OpenAI key and transcription model with each
`start_session` request, so it never restarts the worker when settings change.
When run on its own, set your key before running:

```bash
export OPENAI_API_KEY=...
```

On Windows PowerShell:

```powershell
$env:OPENAI_API_KEY="..."
```

## Packaging

`scripts/build-windows-installer.ps1` builds the worker with PyInstaller
`--onedir` into `apps/desktop/src-tauri/binaries/openwhisper-worker/`, and the
installer ships that whole folder. The worker starts without unpacking Python
and its libraries first, as a single-file build did on every start.

## Refine with AI

When refinement is on, the worker tidies each transcript with `gpt-5-nano`
(`DEFAULT_POLISH_MODEL`; override with `OPENWHISPER_POLISH_MODEL`). The desktop
settings text names the same model, and a desktop test keeps the two equal.
A short transcript (under 12 words, one punctuated sentence, no filler words or
false starts) is pasted as it is, without the extra model call, unless the user
set custom refine instructions.

## Compare transcription models

Run both supported models against the same audio file. Supplying the known transcript adds a
normalized word-accuracy score; multiple runs make the latency comparison less sensitive to
network noise.

```powershell
uv run python scripts/benchmark_transcription_models.py sample.wav `
  --reference "The exact words in sample.wav" `
  --runs 5
```

## Realtime streaming smoke test

Use a 24 kHz mono PCM16 WAV to verify that transcript deltas arrive before the
audio upload finishes:

```powershell
uv run python scripts/smoke_realtime_transcription.py sample.wav
```

The command exits non-zero unless at least one delta arrives before finalization.

