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
    expect(main).toContain('invoke("paste_to_target", { text: delta })');
    expect(main).toContain('invoke("replace_streamed_target"');
    expect(main).toContain("import.meta.env.DEV");
    expect(main).toContain("__openwhisperRealtimeSmoke");
    expect(css).not.toMatch(/\.live-transcript\s*\{/);
  });

  it("asks before installing an update and reassures users that settings are kept", () => {
    const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8");

    expect(main).toContain("window.confirm(");
    expect(main).toContain("Your settings and API key will be kept.");
    expect(main).toContain("await update.downloadAndInstall()");
    expect(main).toContain("await relaunch()");
  });
});
