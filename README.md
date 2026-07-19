# WhisperFlow

WhisperFlow is a cross-platform desktop dictation app that captures live speech, transcribes with OpenAI speech models, and inserts text into the focused input target.

## Run website locally

```bash
cd apps/web
npm install
npm run dev
```

Opens at [http://localhost:3000](http://localhost:3000). For production build: `npm run build && npm start`.

## Run OpenWhisper app locally

**Bash / WSL / Git Bash:**
```bash
./scripts/run-local.sh
```

**PowerShell (Windows):**
```powershell
.\scripts\run-local.ps1
```

**Prerequisites:** [Rust](https://rustup.rs), [uv](https://docs.astral.sh/uv/) (Python), [Node.js](https://nodejs.org). On Linux/WSL, install native deps: `sudo apt-get install -y pkg-config libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf`

On WSL with GPU issues, use software rendering:
```bash
LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe WEBKIT_DISABLE_DMABUF_RENDERER=1 ./scripts/run-local.sh
```

## Rebuild the desktop app

From the repo root, run:

```powershell
cd apps/desktop
npm install
npm run tauri:build
```

That rebuilds the local Tauri desktop app bundle.

## Android companion (Samsung Keyboard compatible)

`apps/android` is a native Kotlin companion app. Android does not provide a
supported way for a third-party app to modify Samsung Keyboard itself, so the
app keeps the selected keyboard and docks a small, user-authorized accessibility
mic beside the keyboard's A key. It records on device, sends a WAV file directly
to the OpenAI audio transcription endpoint, and safely inserts the result into
the field that was focused when recording began.

Download the signed Android beta from the public release page:

**[Download OpenWhisper for Android](https://github.com/JamesHuckle/OpenWhisper/releases/latest/download/OpenWhisper-Android.apk)**

On Windows, the repository can install its own JDK, Android SDK, and API 36
emulator under the ignored `.tools` directory:

```powershell
.\scripts\bootstrap-android.ps1 -WithEmulator
.\scripts\test-android.ps1
```

Build only:

```powershell
.\scripts\android.ps1 assembleDebug
```

APK output: `apps/android/app/build/outputs/apk/debug/app-debug.apk`.
Installation, Samsung device setup, architecture, privacy constraints, and the
test matrix are documented in [`docs/android.md`](docs/android.md).

Maintainers can create and publish a signed update with:

```powershell
.\scripts\setup-android-signing.ps1 # first release only
.\scripts\release-android.ps1
```

To rebuild the full Windows installer instead, run from the repo root:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\build-windows-installer.ps1
```

Installer output: `%USERPROFILE%\openwhisper-cargo-target\release\bundle\nsis\`

This repository uses a hybrid architecture:

- `apps/desktop`: Tauri 2 desktop shell (UI + tray + local control plane)
- `apps/android`: native Android companion and accessibility keyboard overlay
- `apps/worker`: Python worker for OpenAI transcription and text post-processing
- `apps/web`: Next.js marketing site and download page (deployed on Vercel)
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

## Releasing a new version

Releases happen **automatically** when you push to `main`. A Git pre-push hook builds the Windows installer locally and publishes it to GitHub Releases.

**Prerequisites (one-time):**
- [Rust](https://rustup.rs) (`winget install -e --id Rustlang.Rustup`)
- Visual Studio Build Tools (Desktop C++ workload)
- [GitHub CLI](https://cli.github.com) (`winget install --id GitHub.cli`, then `gh auth login`)
- [uv](https://docs.astral.sh/uv/), [Node.js](https://nodejs.org)
- Hook path configured: `git config core.hooksPath scripts/hooks`
- **Updater signing keys** (one-time): `.\scripts\setup-updater-keys.ps1`

**Release workflow:**
```powershell
# 1. Bump version in apps/desktop/src-tauri/tauri.conf.json (and Cargo.toml)
# 2. Commit and push to main — the release happens automatically:
git push origin main
```

When you push to `main`, the pre-push hook will:
1. Build the standalone Python worker executable
2. Build the Tauri Windows NSIS installer (with signed updater artifacts)
3. Tag the commit with the version (e.g. `v0.1.0`)
4. Create/update a GitHub Release marked as **latest**
5. Upload installer, stable-named asset, and `latest.json` (for auto-update)
6. Then the push completes normally

**Auto-update:** Installed apps check for updates on startup. If a newer version exists, they download and install it, then relaunch.

**Skip update notification** (release without notifying existing users): set `OPENWHISPER_SKIP_UPDATE_NOTIFY=1` in `.env`, then push or run the release script.

**Manual release** (alternative to the hook):
```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\release.ps1
```

To skip the release build for a quick push:
```powershell
$env:SKIP_RELEASE = "1"; git push origin main; $env:SKIP_RELEASE = $null
```

Detailed commands and production roadmap are in `docs/architecture.md`.

