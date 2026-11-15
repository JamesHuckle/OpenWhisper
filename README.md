# WhisperFlow

WhisperFlow is a cross-platform desktop dictation app that captures live speech, transcribes with OpenAI speech models, and inserts text into the focused input target.

## Push-to-release contract (read this first)

Every push to `main` runs the release workflow in GitHub Actions
(`.github/workflows/release.yml`). It does not matter which machine pushed: a
Windows PC, a Linux VM and an agent's worktree all release the same way.

1. It runs every test: the website, the Python worker, the desktop's Rust logic
   and UI, the Android unit tests and the release steps themselves.
2. When app code changed since the last release tag (anything under
   `apps/desktop`, `apps/worker`, `apps/android` or `packages/protocol`, or the
   two build scripts, except tests and Markdown), it builds and signs the
   Windows installer and the Android APK. Website-only and docs-only pushes
   stop after the tests.
3. It uploads both builds and both update feeds to a draft GitHub Release,
   checks the draft, publishes it as the latest release and then checks the
   public feeds. The release tag is created only at that point, on the pushed
   commit, so users only ever get code that is on `main`.

The workflow sets the version (the last release tag + 1) in its own copy of the
code, so commits never change the version files. To start a new minor or major
version, raise the version once in a normal commit:
`node scripts/release.mjs set-version 0.3.0` writes it into every desktop and
Android version file.

Vercel deploys `apps/web` from `main`. Users receive updates as follows:

- Windows checks the signed feed at startup, announces the new version, and
  installs it after the user confirms.
- Android 0.2.0 and newer checks when opened and daily while its accessibility
  service is enabled, posts an update notification, verifies the APK, and opens
  Android's required installation confirmation.
- The web app is replaced by the Vercel deployment; visitors receive the new
  version on their next page load.

If a release fails, fix the reported problem and push again, or rerun the
workflow from the Actions tab: a rerun replaces the unfinished draft. Nothing
reaches users until both builds and both feeds are checked.

Machines that used the old local release hooks should remove that setting once:

```bash
git config --unset core.hooksPath
```

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

Releases, including the signed Android APK, are published by the release
workflow (see [Releasing a new version](#releasing-a-new-version)). The Android
signing key is created once with `.\scripts\setup-android-signing.ps1`.

To build the full Windows installer on a Windows PC, run from the repo root:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\build-windows-installer.ps1
```

Installer output: `artifacts\windows-installer\OpenWhisper_x64-setup.exe` (with the
versioned and signed build outputs also kept under
`%USERPROFILE%\openwhisper-cargo-target\release\bundle\nsis\`). The build reads
the updater signing key from `.env`.

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

Releases happen **automatically** in GitHub Actions when a push to `main`
changes app code (see the push-to-release contract above). Releasing needs no
local tools: commit and push normally.

**One-time setup:**

- Vercel project connected to this repository with production branch `main`
  and root directory `apps/web`.
- These repository secrets (Settings, Secrets and variables, Actions). The keys
  must be the ones earlier releases were signed with, or installed apps cannot
  update.

| Secret | Value |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | The updater private key (`%USERPROFILE%\.tauri\openwhisper.key`, created by `.\scripts\setup-updater-keys.ps1`) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Its password, only if it has one |
| `OPENWHISPER_ANDROID_KEYSTORE_BASE64` | The Android keystore file (`OPENWHISPER_ANDROID_KEYSTORE` in `.env`), base64-encoded |
| `OPENWHISPER_ANDROID_KEY_ALIAS` | From `.env` |
| `OPENWHISPER_ANDROID_STORE_PASSWORD` | From `.env` |
| `OPENWHISPER_ANDROID_KEY_PASSWORD` | From `.env` |

Set them with the GitHub CLI so the values are never shown, for example:

```bash
gh secret set TAURI_SIGNING_PRIVATE_KEY < ~/.tauri/openwhisper.key
base64 -w0 openwhisper-release.jks | gh secret set OPENWHISPER_ANDROID_KEYSTORE_BASE64
```

**What a release run does:**

1. Test and production-build the website; test the Python worker, the
   desktop's Rust logic (`cargo test` in `apps/desktop/src-tauri/core-tests`,
   which needs no Tauri), the desktop UI, the Android unit tests and
   `scripts/release.mjs`.
2. Decide whether app code changed since the last tag and choose the version
   (`node scripts/release.mjs plan`).
3. On Windows runners, set that version (`set-version`), then build and sign
   the Tauri NSIS installer and updater signature
   (`scripts/build-windows-installer.ps1`) and, in parallel, test, lint, build,
   sign and verify the Android APK (`scripts/bootstrap-android.ps1`, then
   `scripts/build-android-release.ps1`). The Android `versionCode` comes from
   the version (0.2.28 is 2028), so it grows with every release.
4. Upload the installers, APKs, checksum and both update feeds (`latest.json`
   and `OpenWhisper-Android-update.json`) to a draft release, check that every
   file finished uploading and that both feeds match the builds.
5. Publish the draft as latest, which tags the pushed commit, then re-download
   and check both public feeds and the public APK
   (`node scripts/release.mjs publish`).

**Auto-update:** Installed native apps announce an available update and guide
the user through installation. The desktop app relaunches afterward. Android
requires a system confirmation for sideloaded updates.

Detailed commands and production roadmap are in `docs/architecture.md`.

