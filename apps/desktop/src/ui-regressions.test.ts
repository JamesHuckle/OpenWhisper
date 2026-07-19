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
    expect(main).toContain('visible: showIdlePill || state !== "idle" || menuOpen || manualOverlayReveal');
    expect(native).toContain("window.hide().map_err");
    expect(native).toContain('MenuItem::with_id(app, "show", "Show Pill"');
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
});
