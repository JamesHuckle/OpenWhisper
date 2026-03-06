# WhisperFlow

WhisperFlow is a cross-platform desktop dictation app that captures live speech, transcribes with OpenAI speech models, and inserts text into the focused input target.

## Run locally

```bash
./scripts/run-local.sh
```

**Prerequisites:** [Rust](https://rustup.rs), [uv](https://docs.astral.sh/uv/) (Python), [Node.js](https://nodejs.org). On Linux/WSL, install native deps: `sudo apt-get install -y pkg-config libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf`

On WSL with GPU issues, use software rendering:
```bash
LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe WEBKIT_DISABLE_DMABUF_RENDERER=1 ./scripts/run-local.sh
```

This repository uses a hybrid architecture:

- `apps/desktop`: Tauri 2 desktop shell (UI + tray + local control plane)
- `apps/worker`: Python worker for OpenAI transcription and text post-processing
- `packages/protocol`: shared JSON protocol contract for IPC
- `docs`: architecture and milestone plan

## Why this architecture

- Native desktop reliability for global hotkeys, focus tracking, and text insertion
- Python velocity for model routing and transcription orchestration
- Strict local IPC boundary to keep modules clean and replaceable

## Quick start

1. Create environment file:
   - `cp .env.example .env`
   - `OPENAI_API_KEY` is optional in development. In the desktop app, users can paste their key in Settings.
2. Install Python worker dependencies:
   - `cd apps/worker`
   - `uv sync`
3. Install desktop dependencies:
   - `cd ../desktop`
   - `npm install`
4. Install Rust toolchain (required by Tauri):
   - `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
   - Restart shell, then verify with `cargo --version`.
5. Install Linux native dependencies (Linux/WSL only):
   - `sudo apt-get update`
   - `sudo apt-get install -y pkg-config libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf`
6. Run desktop app:
   - `npm run tauri:dev`
   - On WSL GPU-constrained setups, use software rendering:
     - `LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe WEBKIT_DISABLE_DMABUF_RENDERER=1 npm run tauri:dev`

## Windows single-click installer (release)

Goal: one download, one installer, no Python setup required for end users.

1. Build on a native Windows machine.
   - Required first-time tools:
     - `winget install -e --id Rustlang.Rustup`
     - Visual Studio Build Tools (Desktop C++ workload)
2. Run:
   - `powershell -ExecutionPolicy Bypass -File .\scripts\build-windows-installer.ps1`
3. Share the generated installer from:
   - `apps/desktop/src-tauri/target/release/bundle/nsis/`
   - If run from a `\\wsl$\...` path, the script mirrors to a local Windows temp build path and copies artifacts to `artifacts/windows-installer/`.
   - If corporate security blocks execution from temp paths, pass a stable local folder:
     - `powershell -ExecutionPolicy Bypass -File .\scripts\build-windows-installer.ps1 -LocalBuildRoot C:\Users\<you>\whisperflow-win-build`

The build script packages a standalone `whisperflow-worker.exe`, bundles it in the Tauri app resources, and produces an NSIS installer.

Detailed commands and production roadmap are in `docs/architecture.md`.

