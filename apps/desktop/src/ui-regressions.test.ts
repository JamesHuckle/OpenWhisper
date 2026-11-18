import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

// Text checks for past bugs in code no behaviour test can reach: the page's
// CSS and browser calls in main.ts, and Windows-only or Tauri-bound Rust.
// Everything else is tested by running it (dictation-controller,
// recording-pipeline, target-write, overlay-layout and the Rust core-tests).
const desktopRoot = resolve(import.meta.dirname, "..");
const nativeSourceDir = resolve(desktopRoot, "src-tauri/src");
const read = (path: string) => readFileSync(resolve(desktopRoot, path), "utf8");

describe("desktop regression guards", () => {
  it("draws nothing outside the pill on the transparent overlay window", () => {
    // Bug (fixed in 2f5522e, 36f95e8): drop shadows and backdrop blur drew a dark
    // halo on the see-through window around the pill.
    const css = read("src/styles.css");
    const rule = (selector: string) => {
      const start = css.indexOf(`${selector} {`);
      return start === -1 ? undefined : css.slice(start, css.indexOf("\n}", start));
    };
    const collapsed = rule('#widget[data-expanded="false"] .widget-surface');
    const expanded = [
      rule('#widget[data-expanded="true"] .widget-surface'),
      rule('#widget[data-expanded="true"][data-menu-open="true"] .widget-surface'),
      rule('[data-state="recording"] .widget-surface'),
    ];

    expect(collapsed).toMatch(/background: transparent;[\s\S]*box-shadow: none;/);
    for (const surface of [collapsed, ...expanded]) {
      expect(surface).not.toContain("backdrop-filter");
    }
    // Each expanded surface keeps one shadow, drawn inside its border.
    for (const surface of expanded) {
      const shadows = surface?.match(/box-shadow:[^;]*/g)?.map((shadow) => shadow.replace(/\([^)]*\)/g, "()"));
      expect(shadows).toEqual([expect.stringMatching(/^box-shadow: inset [^,]*$/)]);
    }
  });

  it("shows failures as a pill notice without opening the menu", () => {
    // Bug (fixed in 38cabe0): every failure forced the menu open over the user's work.
    // dictation-controller.test.ts checks failures reach showFailure.
    const main = read("src/main.ts");
    const showFailure = main.slice(
      main.indexOf("function showFailure("),
      main.indexOf("function showWidgetNotice("),
    );

    expect(showFailure).toContain("showWidgetNotice(");
    expect(showFailure).not.toContain("openMenu()");
  });

  it("falls back to the Windows default microphone when the picked one is gone", () => {
    // Bug (fixed in 70e22c6): a switched-off or unplugged microphone failed every recording.
    const main = read("src/main.ts");

    expect(main).toContain('name === "OverconstrainedError" || name === "NotFoundError"');
    expect(main).toContain("return navigator.mediaDevices.getUserMedia({ audio: true });");
  });

  it("installs an update only after the user confirms it", () => {
    // The README promises Windows asks first; an update relaunches the app.
    const main = read("src/main.ts");
    const confirm = main.indexOf("window.confirm(");

    expect(confirm).toBeGreaterThan(-1);
    expect(confirm).toBeLessThan(main.indexOf("await update.downloadAndInstall()"));
  });

  it("runs the recording file commands off the main thread", () => {
    // Bug (fixed in f086fa0): slow disks and antivirus scans froze the pill while
    // these commands ran on the main thread.
    const lib = read("src-tauri/src/lib.rs");

    for (const command of ["recording_begin", "recording_append_chunk", "recording_finish", "recording_list"]) {
      expect(lib).toContain(`#[tauri::command(async)]\nfn ${command}(`);
    }
  });

  it("keeps dictated text out of Windows clipboard history", () => {
    // Bug (fixed in 27f999f): pasting filled Windows clipboard history with dictations.
    expect(read("src-tauri/src/text_inserter.rs")).toContain('"CanIncludeInClipboardHistory"');
  });

  it("writes debug lines through the log file, not only to the console", () => {
    // Bug (fixed in ff78eb6): Windows release builds have no console, so lines printed
    // with eprintln! were lost. debug_log.rs tests the file in core-tests.
    const printing = readdirSync(nativeSourceDir)
      .filter((name) => name.endsWith(".rs") && name !== "debug_log.rs")
      .filter((name) => readFileSync(resolve(nativeSourceDir, name), "utf8").includes("eprintln!"));

    expect(printing).toEqual([]);
  });
});
