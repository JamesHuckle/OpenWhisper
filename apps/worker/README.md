# WhisperFlow Worker

Python worker process that receives JSON requests from desktop over stdin/stdout.

## Run

```bash
uv sync
uv run python -m whisperflow_worker
```

Set your key before running:

```bash
export OPENAI_API_KEY=...
```

On Windows PowerShell:

```powershell
$env:OPENAI_API_KEY="..."
```

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

