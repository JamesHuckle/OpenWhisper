import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const desktopRoot = resolve(import.meta.dirname, "..");
const repositoryRoot = resolve(desktopRoot, "../..");
const nativeSourceDir = resolve(desktopRoot, "src-tauri/src");

// lib.rs and the files split out of it, read together, so a check still
// holds when code moves between them.
const nativeSourceFiles = [
  "lib.rs",
  "overlay_positioner.rs",
  "recordings.rs",
  "text_inserter.rs",
];

function readNativeSource(): string {
  return nativeSourceFiles
    .map((name) => readFileSync(resolve(nativeSourceDir, name), "utf8"))
    .join("\n");
}

describe("desktop regression contracts", () => {
  it("keeps custom Cargo targets out of Tauri's watched source tree", () => {
    const gitignore = readFileSync(resolve(repositoryRoot, ".gitignore"), "utf8");
    const runLocal = readFileSync(resolve(repositoryRoot, "scripts/run-local.ps1"), "utf8");

    expect(gitignore).toMatch(/^target\*\/\s*$/m);
    expect(runLocal).toContain('"target\\desktop-dev"');
    expect(runLocal).not.toContain('"apps\\desktop\\src-tauri\\target');
  });

  it("names the shared tidy model in the refine settings text", () => {
    // models.test.ts and the worker's test_models.py check that both sides
    // read REFINE_MODEL from packages/protocol/models.json.
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");

    expect(main).toContain("refinement pass (${REFINE_MODEL})");
    expect(main).not.toMatch(/const REFINE_MODEL = "/);
  });

  it("keeps the collapsed pill interior fully transparent", () => {
    const css = readFileSync(resolve(import.meta.dirname, "styles.css"), "utf8");
    const collapsedRule = css.match(
      /#widget\[data-expanded="false"\] \.widget-surface \{([\s\S]*?)\n\}/,
    )?.[1];

    expect(collapsedRule).toBeDefined();
    expect(collapsedRule).toContain("border: 1px solid rgba(120, 126, 136, 0.9)");
    expect(collapsedRule).toContain("background: transparent");
    expect(collapsedRule).toContain("box-shadow: none");
    expect(collapsedRule).not.toContain("backdrop-filter");
    expect(collapsedRule).not.toMatch(/box-shadow:[^;]*inset/);
  });

  it("can completely hide the idle pill and recover it from the tray", () => {
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");
    const native = readNativeSource();

    expect(main).toContain("Show pill when idle");
    expect(main.indexOf("Show pill when idle")).toBeLessThan(main.indexOf("Microphone</div>"));
    expect(main).toContain('aria-label="Show pill when idle"');
    // overlay-layout.test.ts checks when the pill is visible (isOverlayVisible).
    expect(main).toContain("visible: isOverlayVisible({");
    expect(native).toContain("window.hide().map_err");
    expect(native).toContain('MenuItem::with_id(app, "show", "Show Pill"');
  });

  it("offers hold-to-talk and press-to-toggle shortcut modes", () => {
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");
    const native = readNativeSource();

    expect(main).toContain("Hold Ctrl+Space to talk");
    expect(main).toContain("press Ctrl+Space once to start listening and again to finish");
    expect(main).toContain("holdToTalk: menuHoldToTalkToggle.checked");
    expect(native).toContain("HOLD_TO_TALK_ENABLED");
    expect(native).toContain("TOGGLE_RECORDING.store(true");
    expect(native).toContain('emit("toggle-recording", ())');
  });

  it("runs the tests of every Rust file that needs no Tauri before a release", () => {
    const coreTests = readFileSync(resolve(desktopRoot, "src-tauri/core-tests/lib.rs"), "utf8");
    const releaseWorkflow = readFileSync(
      resolve(repositoryRoot, ".github/workflows/release.yml"),
      "utf8",
    );
    const tauriFreeTested = readdirSync(nativeSourceDir)
      .filter((name) => name.endsWith(".rs"))
      .filter((name) => {
        const source = readFileSync(resolve(nativeSourceDir, name), "utf8");
        return (
          source.includes("#[cfg(test)]") && !/\btauri\b|crate::|\bkeyring\b/.test(source)
        );
      });

    expect(tauriFreeTested).toContain("worker_connection.rs");
    for (const name of tauriFreeTested) {
      expect(coreTests).toContain(`#[path = "../src/${name}"]`);
    }
    // These files use other app files (crate::), so the scan above skips them.
    for (const name of ["settings.rs", "recordings.rs", "overlay_geometry.rs"]) {
      expect(coreTests).toContain(`#[path = "../src/${name}"]`);
    }
    // core-tests cannot build lib.rs, so tests written there would never run.
    const lib = readFileSync(resolve(nativeSourceDir, "lib.rs"), "utf8");
    expect(lib).not.toContain("#[cfg(test)]");
    expect(releaseWorkflow).toContain("working-directory: apps/desktop/src-tauri/core-tests");
    expect(releaseWorkflow).toContain("run: cargo test --locked");
  });

  it("refreshes the stable local installer after every Windows build", () => {
    const buildScript = readFileSync(
      resolve(repositoryRoot, "scripts/build-windows-installer.ps1"),
      "utf8",
    );

    expect(buildScript).toContain('$StableInstaller = Join-Path $InstallerDir');
    expect(buildScript).toContain('Copy-Item -LiteralPath $VersionedInstaller -Destination $StableInstaller -Force');
    expect(buildScript).toContain('$OutDir = Join-Path $OriginalRepoRoot "artifacts/windows-installer"');
    expect(buildScript).not.toContain("if ($IsUncRepo) {");
  });

  it("releases from GitHub Actions on every main push, after the tests, through a checked draft", () => {
    const workflow = readFileSync(
      resolve(repositoryRoot, ".github/workflows/release.yml"),
      "utf8",
    );
    const releaseSteps = readFileSync(resolve(repositoryRoot, "scripts/release.mjs"), "utf8");
    const job = (name: string) => {
      const start = workflow.indexOf(`\n  ${name}:\n`);
      const next = workflow.slice(start + 1).search(/\n  [a-z-]+:\n/);
      return next === -1 ? workflow.slice(start) : workflow.slice(start, start + 1 + next);
    };

    expect(workflow).toMatch(/on:\n  push:\n    branches: \[main\]/);
    for (const build of ["build-windows", "build-android"]) {
      expect(job(build)).toContain(
        "needs: [plan, test-web, test-worker, test-desktop, test-android]",
      );
      expect(job(build)).toContain("if: needs.plan.outputs.release == 'true'");
      expect(job(build)).toContain("node scripts/release.mjs set-version");
    }
    expect(job("build-windows")).toContain("./scripts/build-windows-installer.ps1");
    expect(job("build-android")).toContain("./scripts/build-android-release.ps1");
    expect(job("publish")).toContain("needs: [plan, build-windows, build-android]");
    expect(job("publish")).toContain("node scripts/release.mjs publish");
    // The draft is checked before it is published as latest.
    const draft = releaseSteps.indexOf('"--draft", "--target"');
    const check = releaseSteps.indexOf("checkFeeds({", draft);
    const publish = releaseSteps.indexOf('"--draft=false", "--latest"');
    expect(draft).toBeGreaterThan(-1);
    expect(check).toBeGreaterThan(draft);
    expect(publish).toBeGreaterThan(check);
  });

  it("never stops Node or Python processes it did not start while building Windows", () => {
    const buildScript = readFileSync(
      resolve(repositoryRoot, "scripts/build-windows-installer.ps1"),
      "utf8",
    );

    expect(buildScript).not.toContain("Get-Process node");
    expect(buildScript).not.toMatch(/Get-Process python\*, pythonw\* [^\n]*\| Stop-Process/);
    expect(buildScript).toContain(".StartsWith($venvAbs)");
  });

  it("builds signed releases without waiting for an interactive key prompt", () => {
    const packageJson = JSON.parse(
      readFileSync(resolve(desktopRoot, "package.json"), "utf8"),
    );

    expect(packageJson.scripts["tauri:build:signed"]).toContain("--ci");
  });

  it("keeps the transparent canvas around the expanded pill clear", () => {
    const css = readFileSync(resolve(import.meta.dirname, "styles.css"), "utf8");
    const expandedRule = css.match(
      /#widget\[data-expanded="true"\] \.widget-surface \{([\s\S]*?)\n\}/,
    )?.[1];
    const hoveredRule = css.match(
      /#widget\[data-expanded="true"\]\[data-hovered="true"\] \.widget-surface,([\s\S]*?)\n\}/,
    )?.[1];
    const recordingRule = css.match(
      /\[data-state="recording"\] \.widget-surface \{([\s\S]*?)\n\}/,
    )?.[1];

    expect(expandedRule).toBeDefined();
    expect(hoveredRule).toBeDefined();
    expect(recordingRule).toBeDefined();
    expect(expandedRule).toContain("box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.06)");
    expect(hoveredRule).toContain("box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.08)");
    expect(recordingRule).toContain("box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.08)");
    expect(expandedRule).not.toContain("backdrop-filter");
    expect(expandedRule?.match(/box-shadow:/g)).toHaveLength(1);
    expect(hoveredRule?.match(/box-shadow:/g)).toHaveLength(1);
    expect(recordingRule?.match(/box-shadow:/g)).toHaveLength(1);
  });

  it("streams realtime transcript deltas into the previously focused field", () => {
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");
    const css = readFileSync(resolve(import.meta.dirname, "styles.css"), "utf8");

    expect(main).not.toContain('id="live-transcript"');
    expect(main).toContain('invoke<TargetWrite>("type_to_target", { text: delta })');
    expect(main).toContain('invoke<TargetWrite>("insert_final_text", { text: finalTranscript })');
    expect(main).toContain("import.meta.env.DEV");
    expect(main).toContain("__openwhisperRealtimeSmoke");
    expect(css).not.toMatch(/\.live-transcript\s*\{/);
  });

  it("writes into the field locked at the start without clipboard-pasting live pieces", () => {
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");
    const lib = readNativeSource();

    // The clipboard is no longer used for live pieces or blind Backspaces.
    expect(main).not.toContain("paste_to_target");
    expect(main).not.toContain("replace_streamed_target");
    expect(lib).not.toContain("fn paste_to_target(");
    expect(lib).not.toContain("fn replace_streamed_target(");
    // Live typing ends for the recording once the native side stops it.
    expect(main).toContain('if (write.outcome !== "typed") {');
    // Focus changes only track; a recording's target is locked separately.
    expect(lib).toContain("text_inserter::track_foreground();");
    expect(lib).toContain("text_inserter::capture_target();");
    expect(lib).toContain("KEYEVENTF_UNICODE");
    expect(lib).toContain("fn paste_restoring_clipboard(");
    expect(lib).toContain('"CanIncludeInClipboardHistory"');
    expect(lib).toContain("CurrentIsPassword()");
  });

  it("receives worker progress as pushed events instead of polling behind a transcription", () => {
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");
    const client = readFileSync(
      resolve(desktopRoot, "src-tauri/src/worker_client.rs"),
      "utf8",
    );
    const lib = readNativeSource();
    const rustEvent = (name: string) =>
      client.match(new RegExp(`pub const ${name}: &str = "([^"]+)";`))?.[1];

    expect(rustEvent("SESSION_EVENT")).toBe("worker-session-event");
    expect(rustEvent("WORKER_EXITED_EVENT")).toBe("worker-exited");
    expect(main).toContain('listen<WorkerSessionEvent>("worker-session-event"');
    expect(main).toContain('listen<string>("worker-exited"');
    expect(main).not.toContain("worker_poll_session_events");
    expect(main).not.toContain("setInterval(() => void pollOnce()");
    expect(lib).not.toContain("poll_session_events");
    // The worker lock is held only to find or start the worker, so Cancel
    // and settings saves do not wait for a running transcription.
    expect(lib).toContain("Mutex<Option<Arc<WorkerClient>>>");
  });

  it("waits for Windows events instead of polling shortcuts and the focused program", () => {
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");
    const lib = readNativeSource();

    // Shortcuts and focus changes wake a dispatcher; no 20 ms loop remains.
    expect(lib).not.toContain("Duration::from_millis(20)");
    expect(lib).toContain("while woken.recv().is_ok()");
    expect(lib).toContain("SetWinEventHook(");
    expect(lib).toContain("EVENT_SYSTEM_FOREGROUND, EVENT_OBJECT_FOCUS");
    // Only the monitor check repeats, on its own thread, every 100 ms.
    expect(lib).toContain("std::thread::sleep(MONITOR_FOLLOW_INTERVAL)");
    expect(lib).toContain("Duration::from_millis(100)");
    // The menu hears the program name when it changes instead of every 500 ms.
    expect(lib).toContain('emit("foreground-app-changed", name)');
    expect(main).toContain('listen<string | null>("foreground-app-changed"');
    expect(main).not.toContain("pollForegroundApp");
    expect(main).not.toMatch(/setInterval\([^)]*get_foreground_app/);
  });

  it("ships the worker as a folder build so it starts without unpacking", () => {
    const buildScript = readFileSync(
      resolve(repositoryRoot, "scripts/build-windows-installer.ps1"),
      "utf8",
    );
    const tauriConfig = JSON.parse(
      readFileSync(resolve(desktopRoot, "src-tauri/tauri.conf.json"), "utf8"),
    );
    const client = readFileSync(
      resolve(desktopRoot, "src-tauri/src/worker_client.rs"),
      "utf8",
    );

    expect(buildScript).toContain("--onedir");
    expect(buildScript).not.toContain("--onefile");
    // A plain "binaries/*" skips folders, which would leave out the worker.
    expect(tauriConfig.bundle.resources).toEqual(["binaries/**/*"]);
    expect(client).toContain('"binaries/openwhisper-worker/openwhisper-worker.exe"');
  });

  it("asks before installing an update and reassures users that settings are kept", () => {
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");

    expect(main).toContain("window.confirm(");
    expect(main).toContain("Your settings and API key will be kept.");
    expect(main).toContain("await update.downloadAndInstall()");
    expect(main).toContain("await relaunch()");
  });

  it("can retranscribe saved recordings without replacing their raw audio", () => {
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");
    const native = readNativeSource();

    expect(main).toContain('transcribeButton.textContent = "Transcribe"');
    expect(main).toContain("retranscribeRecording(recording)");
    expect(main).toContain('status: "transcribing"');
    // Only the recording id crosses to the app; the worker reads the saved
    // file from disk, so the audio is never loaded into the window.
    expect(main).toContain('invoke("recording_transcribe", { recordingId, sessionId })');
    expect(main).not.toContain('"recording_read"');
    expect(native).not.toContain("fn recording_read(");
    expect(native).toContain("async fn recording_transcribe(");
    expect(native).toContain('"audio_path": audio_path.to_string_lossy()');
  });

  it("sends each piece of normal dictation once, to the saved backup", () => {
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");
    const native = readNativeSource();

    // Only live transcription streams audio to the worker while recording.
    expect(main).toContain("if (streamAudioToWorker) audioQueue.append(combined.buffer);");
    expect(main).toContain(
      "streamAudioToWorker = isLiveTranscription(settings.transcriptionModel);",
    );
    // The backup file stays open and its header is not rewritten per piece.
    expect(native).toContain("recordings: std::sync::Mutex<HashMap<String, ActiveRecording>>");
    expect(native).not.toContain("fn append_pcm_chunk(");
  });

  it("imports audio files by copying them on disk, not through the window", () => {
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");
    const native = readNativeSource();

    expect(main).not.toContain('type="file"');
    expect(main).not.toContain(".arrayBuffer()");
    expect(main).toContain('invoke<{ id: string } | null>("recording_import")');
    expect(native).toContain(".blocking_pick_file()");
    expect(native).toContain("fs::copy(source, dir.join(&file_name))");
  });

  it("opens the microphone without waiting on settings, files or a new audio context", () => {
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");
    const lib = readFileSync(resolve(nativeSourceDir, "lib.rs"), "utf8");
    const beginBody = lib.slice(
      lib.indexOf("fn recording_begin("),
      lib.indexOf("fn recording_append_chunk("),
    );

    // One capture context, reused; the volume monitor listens on it too.
    expect(main.match(/new AudioContext\(/g)).toHaveLength(1);
    expect(main).toContain("await context.suspend();");
    expect(main).not.toContain("context.close();\n  micStream");
    // Settings come from memory; the app is asked only when they are unknown.
    expect(main.match(/invoke<AppSettings>\("app_get_settings"\)/g)).toHaveLength(2);
    expect(main).toContain('openBackup: () => invoke<{ id: string }>("recording_begin"');
    // File work runs off the main thread, and starting a recording prunes nothing.
    for (const command of [
      "recording_begin",
      "recording_append_chunk",
      "recording_finish",
      "recording_list",
    ]) {
      expect(lib).toContain(`#[tauri::command(async)]\nfn ${command}(`);
    }
    expect(lib).toContain("spawn_blocking(move || import_recording(&root, &source))");
    expect(beginBody).not.toContain("prune_recordings");
  });

  it("records only through the PCM worklet, so a Live finalize always names PCM", () => {
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");
    const finalizeBody = main.slice(
      main.indexOf("async function finalizeRecording("),
      main.indexOf("async function", main.indexOf("async function finalizeRecording(") + 1),
    );

    expect(main).not.toContain("MediaRecorder");
    expect(main).not.toContain("recordingMimeType");
    expect(finalizeBody).toContain("mimeType: PCM_MIME_TYPE,");
  });

  it("starts the Python worker from one place and keeps no unused worker commands", () => {
    const lib = readFileSync(resolve(nativeSourceDir, "lib.rs"), "utf8");
    const client = readFileSync(resolve(nativeSourceDir, "worker_client.rs"), "utf8");

    expect(lib).not.toContain("worker_list_models");
    expect(client.match(/\.arg\("openwhisper_worker"\)/g)).toHaveLength(1);
    expect(client.match(/try_spawn_python\(&worker_src\)/g)).toHaveLength(2);
  });

  it("promotes saved transcripts over redundant retranscription", () => {
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");

    expect(main).toContain("Recordings &amp; transcripts");
    expect(main).toContain("if (transcript) {");
    expect(main).toContain('invoke("copy_to_clipboard", { text: transcript })');
    // A non-empty transcript must render the transcript surface, not Transcribe.
    expect(main.indexOf("panel.append(surface, transcriptToggle)"))
      .toBeLessThan(main.indexOf("} else {", main.indexOf("if (transcript) {")));
  });

  it("renders one accessible transcript surface for both preview and full text", () => {
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");
    const css = readFileSync(resolve(import.meta.dirname, "styles.css"), "utf8");

    // Single text surface — the same element is clamped when collapsed and
    // revealed when expanded, never duplicated into a second block.
    expect(main).toContain("recording-transcript-text");
    expect(main).toContain("transcriptText.id = `recording-transcript-${recording.id}`");
    expect(main).toContain('transcriptText.textContent = transcript');
    expect(main).not.toContain("recording-transcript-body");
    expect(main).not.toContain("transcriptPreview");

    // Disclosure control drives expansion via aria-expanded/aria-controls and
    // is a sibling of the copy button (no button nested inside a button).
    expect(main).toContain('transcriptToggle.setAttribute("aria-expanded", "false")');
    expect(main).toContain('transcriptToggle.setAttribute("aria-controls", transcriptText.id)');
    expect(main).toContain('panel.dataset.expanded = String(next)');

    // Compact icon-only copy control: accessible name + title, no "Copy" text.
    expect(main).toContain('copyButton.setAttribute("aria-label", `Copy transcript for ${recording.name}`)');
    expect(main).toContain('copyButton.title = "Copy transcript"');
    expect(main).not.toContain('copyButton.textContent = "Copy"');

    // CSS drives the collapsed preview via line-clamp + fade on one surface.
    expect(css).toContain("-webkit-line-clamp: 3");
    expect(css).toContain('.recording-transcript[data-expanded="false"][data-clamped="true"] .recording-transcript-text');
    expect(css).toContain("mask-image: linear-gradient(to bottom");
    expect(css).not.toContain(".recording-transcript-body");
  });

  it("reports failures with a brief inline pill notice instead of opening the menu", () => {
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");
    const css = readFileSync(resolve(import.meta.dirname, "styles.css"), "utf8");
    const failureHandler = main.slice(
      main.indexOf("async function handleTranscriptionFailure"),
      main.indexOf("function showWidgetNotice"),
    );

    expect(failureHandler).toContain("showWidgetNotice(");
    expect(failureHandler).not.toContain("openMenu()");
    expect(failureHandler).not.toContain("showToast(");
    expect(main).toContain("const NOTICE_DURATION_MS = 5000;");
    expect(main).toContain('id="widget-notice"');
    expect(css).toContain('#widget[data-notice="true"] .widget-notice');
  });
});
