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

