import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const desktopRoot = resolve(import.meta.dirname, "..");
const repositoryRoot = resolve(desktopRoot, "../..");

describe("desktop regression contracts", () => {
  it("keeps custom Cargo targets out of Tauri's watched source tree", () => {
    const gitignore = readFileSync(resolve(repositoryRoot, ".gitignore"), "utf8");
    const runLocal = readFileSync(resolve(repositoryRoot, "scripts/run-local.ps1"), "utf8");

    expect(gitignore).toMatch(/^target\*\/\s*$/m);
    expect(runLocal).toContain('"target\\desktop-dev"');
    expect(runLocal).not.toContain('"apps\\desktop\\src-tauri\\target');
  });

  it("names the worker's default tidy model in the refine settings text", () => {
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");
    const worker = readFileSync(
      resolve(repositoryRoot, "apps/worker/src/openwhisper_worker/openai_client.py"),
      "utf8",
    );
    const workerModel = worker.match(/^DEFAULT_POLISH_MODEL = "([^"]+)"$/m)?.[1];
    const desktopModel = main.match(/^const REFINE_MODEL = "([^"]+)";$/m)?.[1];

    expect(workerModel).toBe("gpt-5-nano");
    expect(desktopModel).toBe(workerModel);
    expect(main).toContain("refinement pass (${REFINE_MODEL})");
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
    const native = readFileSync(resolve(desktopRoot, "src-tauri/src/lib.rs"), "utf8");

    expect(main).toContain("Show pill when idle");
    expect(main.indexOf("Show pill when idle")).toBeLessThan(main.indexOf("Microphone</div>"));
    expect(main).toContain('aria-label="Show pill when idle"');
    expect(main).toContain('visible: showIdlePill || state !== "idle" || menuOpen || manualOverlayReveal');
    expect(native).toContain("window.hide().map_err");
    expect(native).toContain('MenuItem::with_id(app, "show", "Show Pill"');
  });

  it("offers hold-to-talk and press-to-toggle shortcut modes", () => {
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");
    const native = readFileSync(resolve(desktopRoot, "src-tauri/src/lib.rs"), "utf8");

    expect(main).toContain("Hold Ctrl+Space to talk");
    expect(main).toContain("press Ctrl+Space once to start listening and again to finish");
    expect(main).toContain("holdToTalk: menuHoldToTalkToggle.checked");
    expect(native).toContain("HOLD_TO_TALK_ENABLED");
    expect(native).toContain("TOGGLE_RECORDING.store(true");
    expect(native).toContain('emit("toggle-recording", ())');
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

  it("updates the installed Windows app during every normal main push", () => {
    const releaseScript = readFileSync(
      resolve(repositoryRoot, "scripts/pre-push-release.ps1"),
      "utf8",
    );
    const syncScript = readFileSync(
      resolve(repositoryRoot, "scripts/sync-local-windows-release.ps1"),
      "utf8",
    );

    expect(releaseScript).toContain('sync-local-windows-release.ps1');
    expect(releaseScript).toContain('-InstallerPath $versionedInstaller');
    expect(releaseScript).toContain('-ExpectedVersion $version');
    expect(syncScript).toContain('VersionInfo.ProductVersion');
    expect(syncScript).toContain('Start-Process -FilePath $resolvedInstaller -ArgumentList "/S" -Wait');
    expect(syncScript).toContain('Local\\OpenWhisperLocalReleaseInstall');
    expect(syncScript).toContain('Start-Process -FilePath $installedExecutable');
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
    const lib = readFileSync(resolve(desktopRoot, "src-tauri/src/lib.rs"), "utf8");

    // The clipboard is no longer used for live pieces or blind Backspaces.
    expect(main).not.toContain("paste_to_target");
    expect(main).not.toContain("replace_streamed_target");
    expect(lib).not.toContain("fn paste_to_target(");
    expect(lib).not.toContain("fn replace_streamed_target(");
    // Live typing ends for the recording once the native side stops it.
    expect(main).toContain('if (write.outcome !== "typed") {');
    // The 20 ms loop only tracks; a recording's target is locked separately.
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
    const lib = readFileSync(resolve(desktopRoot, "src-tauri/src/lib.rs"), "utf8");
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
    const native = readFileSync(resolve(desktopRoot, "src-tauri/src/lib.rs"), "utf8");

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
    const native = readFileSync(resolve(desktopRoot, "src-tauri/src/lib.rs"), "utf8");

    // Only live transcription streams audio to the worker while recording.
    expect(main).toContain("if (streamAudioToWorker) audioQueue.append(combined.buffer);");
    expect(main).toContain(
      'streamAudioToWorker = settings.transcriptionModel === "gpt-live-transcribe";',
    );
    // The backup file stays open and its header is not rewritten per piece.
    expect(native).toContain("recordings: std::sync::Mutex<HashMap<String, ActiveRecording>>");
    expect(native).not.toContain("fn append_pcm_chunk(");
  });

  it("imports audio files by copying them on disk, not through the window", () => {
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");
    const native = readFileSync(resolve(desktopRoot, "src-tauri/src/lib.rs"), "utf8");

    expect(main).not.toContain('type="file"');
    expect(main).not.toContain(".arrayBuffer()");
    expect(main).toContain('invoke<{ id: string } | null>("recording_import")');
    expect(native).toContain(".blocking_pick_file()");
    expect(native).toContain("fs::copy(source, dir.join(&file_name))");
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
