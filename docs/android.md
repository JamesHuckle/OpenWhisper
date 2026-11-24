# Android companion

## What was built

OpenWhisper now has a native Kotlin Android app in `apps/android`, targeting
Android API 36 with a minimum of API 28. It does not replace or patch Samsung
Keyboard. Android has no supported extension point for injecting controls into
another vendor's IME. Instead, a user-enabled `AccessibilityService` owns a
small `TYPE_ACCESSIBILITY_OVERLAY` microphone that follows the active keyboard.

This gives the intended S26 Ultra experience while leaving Samsung Keyboard as
the selected IME:

1. Focus a normal editable field and let Samsung Keyboard appear.
2. OpenWhisper docks its compact mic key in the left gutter beside the A key.
3. Tap once to record and again to finish.
4. The transcript is inserted at the captured cursor or replaces only the
   captured selection.

The control is hidden when no keyboard is visible, focus is not editable, or
the platform marks a field as password/sensitive.

Some rich-text apps, including OneNote, place their editor in an embedded or
virtual view hierarchy. Android's direct input-focus lookup can return `null`
for those hierarchies even while the keyboard is visible. OpenWhisper falls
back to traversing the interactive application windows for the focused editor
and also accepts custom editors that expose the platform's set-text action
without setting the redundant editable flag. Each overlay refresh checks for
the keyboard first and looks for the focused editor only while a keyboard is
open, because the control is hidden (or floats during dictation) without one
whatever has focus. The window traversal visits at most 2,000 nodes, since
each node is a slow call into the other app.

On Android 13 and newer, OpenWhisper also registers as an accessibility input
method editor. If a rich-text app provides a real keyboard `InputConnection`
but no editable accessibility node, OpenWhisper uses that same connection to
detect the editor and commit only the dictated text at its current selection.
Password input types remain suppressed, and focus or surrounding-text changes
during dictation still cancel insertion.

## Architecture and safety boundaries

- `OpenWhisperAccessibilityService` observes focus and IME windows, owns the
  overlay, and coordinates dictation.
- `EditorSessionController` snapshots stable editor identity, text, selection,
  password state, and sensitivity. Commit is refused if focus or content
  changed during dictation.
- `AndroidAudioRecorder` captures mono 16 kHz PCM and `WavEncoder` writes every
  WAV header: the saved recording's and each WAV upload's. While recording,
  the WAV header is brought up to date about once a second, without waiting
  for storage on the microphone thread, so a killed app still leaves a
  playable file; the file is forced to disk once, when the recording
  finishes. The recording's `metadata.json` is written only when it starts
  and closes, and meanwhile `RecordingStore` reads its size from the WAV
  file, as the desktop does. Each recording gets its own handle: a stop
  queued for one recording never ends the next, stop or cancel cleans it up
  once, and the microphone stays taken until that clean-up finishes.
- `FlacFileEncoder` turns each upload part into lossless FLAC with the phone's
  own `MediaCodec` encoder, about half the bytes of WAV for speech, reading
  the part from the saved file in blocks. If the
  phone cannot encode FLAC the part goes as WAV, and if the service refuses
  the FLAC (HTTP 400) the part is sent once more as WAV and later parts skip
  FLAC.
- `OpenAiHttpTranscriptionClient` sends multipart form data directly to
  `POST /v1/audio/transcriptions` using the shared default model
  (`default_transcription_model` in `packages/protocol/models.json`, read
  into `BuildConfig` at build time). Each part is copied
  from the saved file into the request in 64 KB blocks (behind a generated
  WAV header when it goes as WAV), so a long recording is never held in
  memory; an imported compressed file goes as it is, under its own name and
  type. A part that never reaches the silence level is not uploaded; it is
  saved as finished and empty, so a retry skips it too. A request that
  fails because the connection dropped or the service answered 429, 500,
  502, 503 or 504 is sent again after 1 s and then 3 s (a rate limit's
  Retry-After, up to 10 s, sets the wait), and a cancel ends the wait. The
  wait for an answer once the upload is sent is two minutes plus a second
  per 50 kB uploaded. This request
  shape and model are supported by the current
  [OpenAI transcription API reference](https://developers.openai.com/api/reference/resources/audio/subresources/transcriptions/methods/create).
- `TranscriptionManager`, held by `OpenWhisperApplication`, runs every
  upload, for the app screen and the dictation overlay alike. Each job has an
  origin: a dictation from the overlay runs on its own thread, and imports and
  retries from the app screen run one at a time on another, so a keyboard
  dictation never waits behind a long import. Jobs are keyed by recording:
  asking for a recording that is already being transcribed returns its job
  (the app screen joins a running dictation), so it is never uploaded (and
  paid for) twice. Each job has its own HTTP client, so cancelling one stops only
  its upload, even before it starts. The manager saves each recording's
  status. When Android recreates the app screen the upload carries on and the
  new screen shows the result. Nothing resumes by itself: a job lost with the
  app's process is marked failed when the app next starts, its finished parts
  kept, and the user taps Transcribe to send only the rest.
- `AudioRules` follows the same dictation rules as the desktop, from
  `packages/protocol/audio-rules.json` (see `packages/protocol/README.md`). An
  accidental tap or a muted microphone is never uploaded, and the user sees
  "No audio found", as on the desktop; so do an empty transcript and a part
  that only repeats its prompt. Long recordings are split at the same upload
  size as the desktop, and their parts are joined by the same rules: each
  later part is prompted with the end of the text before it, and dropped when
  its text only repeats that prompt. `AudioRulesTest`, `LongAudioChunkingTest`
  and the desktop's Rust tests run the same examples in
  `packages/protocol/audio-examples`.
- `SecureApiKeyStore` encrypts a user-supplied API key with AES-256-GCM using a
  non-exportable Android Keystore key. App backup and device transfer are
  disabled. This protects the key at rest, but a rooted/compromised phone is
  outside the threat model; a managed public release should use a server-side
  token broker instead of distributing an organization key to phones.
- The accessibility and app UI share one process so settings and encrypted
  credentials remain consistent. No transcript text is written to logs.

The debug build defaults to deterministic demo transcription, which requires
no microphone, key, network, or billable request. Turn that switch off in the
app to exercise live capture.

## Reproducible test environment

From a Windows PowerShell prompt at the repository root:

```powershell
.\scripts\bootstrap-android.ps1 -WithEmulator
.\scripts\test-android.ps1
```

The bootstrap is repository-local: Temurin JDK 17, the Android command-line
tools, API 36, build tools, Gradle, and a Pixel 9 Pro XL API 36 emulator are
stored below ignored `.tools` paths.

The test command performs a clean build and runs:

- JVM tests for cursor/selection preservation, password/sensitive rejection,
  stale focus/content rejection, overlay placement, state transitions,
  cancellation, one upload per recording, error recovery, WAV encoding, FLAC
  upload with WAV fallback, multipart requests, response parsing, and microphone shutdown races;
- Android Keystore instrumentation tests proving encrypted round-trip, clear,
  and absence of plaintext in preferences;
- a main-screen instrumentation test checking each label's size, colour and
  spacing, that buttons keep their written capitalisation, and a saved
  recording's open and transcribe buttons;
- an offline end-to-end device test that enables the accessibility service,
  launches a real editable activity and software keyboard, finds and taps the
  actual overlay window, inserts the deterministic transcript, then proves the
  overlay disappears for a password field;
- Android lint and debug APK assembly.

Useful individual commands:

```powershell
.\scripts\android-emulator.ps1 start -Visible
.\scripts\android.ps1 testDebugUnitTest
.\scripts\android.ps1 connectedDebugAndroidTest
.\scripts\android.ps1 lintDebug assembleDebug
.\scripts\android-emulator.ps1 stop
```

## Install on a Galaxy S26 Ultra

The public beta is a signed, non-debuggable APK:

**[Download OpenWhisper-Android.apk](https://github.com/JamesHuckle/OpenWhisper/releases/latest/download/OpenWhisper-Android.apk)**

Download that file on the phone and open it from **My Files**. Samsung Auto
Blocker may need to be disabled temporarily under **Settings > Security and
privacy > Auto Blocker**. Allow the browser or My Files to install unknown apps
when prompted, then re-enable Auto Blocker after installation.

For a developer installation instead:

1. Build the debug APK with `.\scripts\android.ps1 assembleDebug`.
2. Enable Developer options and USB debugging on the phone.
3. Install with:

   ```powershell
   .\.tools\android-sdk\platform-tools\adb.exe install -r `
     .\apps\android\app\build\outputs\apk\debug\app-debug.apk
   ```

4. Open OpenWhisper and grant microphone and notification permissions.
5. Tap **Enable accessibility overlay**, select **OpenWhisper dictation**, read
   the Android warning, and enable it only if you accept the access described
   above.
6. Keep Samsung Keyboard selected. Leave deterministic demo enabled for the
   first acceptance pass; open the built-in end-to-end test screen and verify
   mic, insertion, and password suppression.
7. Save a personal OpenAI API key, disable demo mode, and repeat with live
   speech.

## Publish a signed GitHub release APK

The first release needs a persistent Android signing key:

```powershell
.\scripts\setup-android-signing.ps1
```

The keystore is created outside the repository at
`%USERPROFILE%\.openwhisper\android\openwhisper-release.jks`; its credentials
are stored in the ignored `.env`. Back up both securely. Losing the key makes
it impossible to publish an update that existing Android installations accept.
Copy the keystore (base64-encoded) and its credentials into the repository
secrets listed in the README under "Releasing a new version".

Releases are built by the GitHub Actions release workflow
(`.github/workflows/release.yml`) on every push to `main` that changes app
code. It builds, lints, signs and verifies the APK with
`scripts/build-android-release.ps1`, uploads it to a draft release together
with the Windows build, checks both update feeds, publishes the release and
re-downloads the public APK to check its SHA-256 digest.

Every release publishes both a versioned APK and the stable
`OpenWhisper-Android.apk` asset used by the latest-download URL. Windows and
Android are always released together, so the shared GitHub `releases/latest`
URLs work for both platforms.

It also publishes `OpenWhisper-Android-update.json`. Android 0.2.0 and newer
checks that feed daily while the accessibility service is enabled and whenever
the app is opened. A daily check that fails (no network, an HTTP error, a bad
feed) only logs one line and tries again the next day; it never stops dictation.
A system update notification opens the app, which fetches the feed again and
shows the in-app prompt for the latest release (or the update-failed message,
with the Check for updates button to retry, if that fetch fails);
one tap downloads the APK, verifies its SHA-256 digest, and opens Android's installer.
Android still requires the user to approve the install (and, on first use, to
allow OpenWhisper as an install source). Silent sideloaded updates are not
permitted by the platform.

Updates use the same `com.openwhisper.android` package and persistent signing
key, so Android performs an in-place upgrade. Shared preferences, the Android
Keystore entry, and the encrypted OpenAI API key remain untouched. Never change
the application ID or release key. The release workflow sets `versionCode`
from the version (0.2.28 is 2028), so it grows with every release.

The original 0.1.0 build predates this updater and cannot be made to display a
remote prompt retroactively. Those users need to install 0.2.0 manually once;
all later releases can notify them in-app.

The automated environment validates Android 16/API 36 framework behavior and a
real software IME. A physical S26 Ultra/One UI pass is still required before
claiming Samsung-specific release certification, particularly for keyboard
geometry, battery restrictions, microphone foreground-service behavior, and
rotation/multi-window layouts.

## APKs and reports

- Debug APK: `apps/android/app/build/outputs/apk/debug/app-debug.apk`
- Signed public APK: `apps/android/app/build/outputs/apk/public/OpenWhisper-Android.apk`
- JVM report: `apps/android/app/build/reports/tests/testDebugUnitTest/index.html`
- Device report: `apps/android/app/build/reports/androidTests/connected/debug/index.html`
- Lint report: `apps/android/app/build/reports/lint-results-debug.html`

See `docs/android-acceptance.md` for the release acceptance checklist.
