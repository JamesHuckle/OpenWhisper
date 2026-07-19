# WhisperFlow

WhisperFlow is a cross-platform desktop dictation app that captures live speech, transcribes with OpenAI speech models, and inserts text into the focused input target.

## Push-to-release contract (read this first)

Every normal `git push origin main` is a coordinated production release of
the web, Windows, and Android apps. Configure the repository hooks once with:

```powershell
git config core.hooksPath scripts/hooks
```

After that, the supported workflow is simply:

```powershell
git add -A
git commit -m "Describe the change"
git push origin main
```

The commit hook assigns one new version to both native apps and increments the
Android `versionCode`. The push hook requires a clean tree, runs the web,
worker, desktop, and Android test/build checks, creates signed Windows and
Android packages, publishes both update feeds to one GitHub Release, and only
then allows the `main` push to finish. Vercel deploys `apps/web` from `main`.

Users receive updates as follows:

- Windows checks the signed feed at startup, announces the new version, and
  installs it after the user confirms.
- Android 0.2.0 and newer checks when opened and daily while its accessibility
  service is enabled, posts an update notification, verifies the APK, and opens
  Android's required installation confirmation.
- The web app is replaced by the Vercel deployment; visitors receive the new
  version on their next page load.

A native release is deliberately a full signed rebuild on every push, even for
a documentation-only change. Do not use `--no-verify`, `SKIP_RELEASE`, or
`OPENWHISPER_SKIP_VERSION_BUMP` for a production push: those escape hatches
break the guarantee that all apps share the release. If a hook fails, fix the
reported problem and rerun the normal commit or push; a retry safely repairs
the same release.

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

Android 0.2.0 and newer checks for signed releases in the background, notifies
the user, verifies the downloaded APK, and opens Android's normal update
confirmation. In-place updates keep existing settings and the encrypted API
key. Users on the original 0.1.0 beta must install 0.2.0 manually once to gain
the updater.

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

Installer output: `artifacts\windows-installer\OpenWhisper_x64-setup.exe` (with the
versioned and signed build outputs also kept under
`%USERPROFILE%\openwhisper-cargo-target\release\bundle\nsis\`).

Every normal push to `main` runs the local release hook. After the signed
release and public update feeds are verified, the hook silently updates the
installed `%LOCALAPPDATA%\OpenWhisper` copy to the same version and restarts it
only when it was already running. `SKIP_RELEASE=1` intentionally bypasses both
the release and this local installation sync.

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

Releases happen **automatically** when you commit and push to `main`. The
repository hooks synchronize versions, test all components, build both native
apps locally, and publish one coordinated GitHub Release.

**Prerequisites (one-time):**
- [Rust](https://rustup.rs) (`winget install -e --id Rustlang.Rustup`)
- Visual Studio Build Tools (Desktop C++ workload)
- [GitHub CLI](https://cli.github.com) (`winget install --id GitHub.cli`, then `gh auth login`)
- [uv](https://docs.astral.sh/uv/), [Node.js](https://nodejs.org)
- Hook path configured: `git config core.hooksPath scripts/hooks`
- **Updater signing keys** (one-time): `.\scripts\setup-updater-keys.ps1`
- **Android signing key** (one-time): `.\scripts\setup-android-signing.ps1`
- Vercel project connected to this repository with production branch `main`
  and root directory `apps/web`

**Release workflow:**
```powershell
# Versions are assigned by the commit hook. Commit and push normally:
git add -A
git commit -m "Describe the change"
git push origin main
```

When you push to `main`, the pre-push hook will:

1. Test and production-build the website.
2. Test the Python worker and desktop UI.
3. Test, lint, build, sign, and verify the Android APK.
4. Build and sign the Tauri Windows NSIS installer and updater artifact.
5. Tag the commit, create/update the GitHub Release, and publish both update
   feeds and stable download assets.
6. Re-download and verify the public Android APK and both feed versions.
7. Allow the push to complete, after which Vercel deploys the website.

**Auto-update:** Installed native apps announce an available update and guide
the user through installation. The desktop app relaunches afterward. Android
requires a system confirmation for sideloaded updates.

Detailed commands and production roadmap are in `docs/architecture.md`.

