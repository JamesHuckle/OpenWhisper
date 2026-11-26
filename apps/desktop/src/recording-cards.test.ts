// @vitest-environment jsdom
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

// Renders the real menu page (main.ts) headless with the Tauri bridge faked,
// opens the menu and checks the recent recording cards it draws.
const recordings = [
  {
    id: "rec-1",
    createdAtMs: Date.UTC(2026, 10, 26, 6, 0),
    name: "Morning note",
    source: "microphone",
    mimeType: "audio/wav",
    fileName: "rec-1.wav",
    status: "transcribed",
    bytes: 3 * 1024 * 1024,
    transcript: "  Buy milk and call Sam.  ",
    error: "",
  },
  {
    id: "rec-2",
    createdAtMs: Date.UTC(2026, 10, 25, 18, 30),
    name: "Imported <b>call</b>",
    source: "import",
    mimeType: "audio/mpeg",
    fileName: "rec-2.mp3",
    status: "failed",
    bytes: 200,
    transcript: "   ",
    error: "network",
  },
];
let listed: typeof recordings = recordings;
let holdTranscription = false;

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("@tauri-apps/plugin-autostart", () => ({ enable: vi.fn(), isEnabled: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn() }));

const settled = () => new Promise((resolve) => setTimeout(resolve, 20));
const cards = () => document.getElementById("recent-recordings")!;
const when = (ms: number) =>
  new Date(ms).toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });

// Tag, sorted attributes and text of every node, one per line; whitespace-only
// text between elements is not drawn, so it is left out.
function tree(node: Node, depth = 0): string[] {
  const pad = "  ".repeat(depth);
  if (node.nodeType === Node.TEXT_NODE) {
    const text = node.textContent!.trim();
    return text ? [`${pad}"${text}"`] : [];
  }
  const element = node as Element;
  const attributes = [...element.attributes]
    .map((attribute) => `${attribute.name}="${attribute.value}"`)
    .sort()
    .join(" ");
  const hidden = element instanceof HTMLElement && element.hidden ? " (hidden)" : "";
  const disabled = element instanceof HTMLButtonElement && element.disabled ? " (disabled)" : "";
  return [
    `${pad}<${element.tagName.toLowerCase()}${attributes ? ` ${attributes}` : ""}>${hidden}${disabled}`,
    ...[...element.childNodes].flatMap((child) => tree(child, depth + 1)),
  ];
}

async function openMenuWith(list: typeof recordings) {
  listed = list;
  await (window as unknown as { __openWhisperTest: { openMenu: () => Promise<void> } }).__openWhisperTest.openMenu();
  await settled();
}

beforeAll(async () => {
  invoke.mockImplementation(async (command: string) => {
    if (command === "recording_list") return listed;
    if (command === "app_get_settings") {
      return {
        hasOpenaiApiKey: true,
        openaiApiKeyPreview: "sk-...abcd",
        transcriptionModel: "gpt-4o-transcribe",
        transcriptionPrompt: "",
        refineEnabled: false,
        refinePrompt: "",
        showIdlePill: true,
        holdToTalk: true,
      };
    }
    if (command === "get_foreground_app") return null;
    if (holdTranscription && command === "worker_start_session") return new Promise(() => {});
    return undefined;
  });
  Object.defineProperty(navigator, "mediaDevices", {
    value: { getUserMedia: vi.fn(async () => ({ getTracks: () => [] })), enumerateDevices: vi.fn(async () => []) },
  });
  document.body.innerHTML = '<div id="app"></div>';
  await import("./main");
  await settled();
});

beforeEach(() => invoke.mockClear());

describe("recent recording cards", () => {
  it("draws a transcript card and a Transcribe card with their classes and labels", async () => {
    await openMenuWith(recordings);

    expect(tree(cards()).join("\n")).toBe(
      [
        '<div class="recent-recordings" id="recent-recordings">',
        '  <div class="recording-card">',
        '    <button class="recording-row" title="Open raw audio" type="button">',
        '      <span class="recording-row-main">',
        '        "Morning note"',
        '      <span class="recording-row-meta">',
        `        "${when(recordings[0].createdAtMs)} | 3.0 MB | transcribed"`,
        '    <div class="recording-transcript" data-clamped="false" data-expanded="false">',
        '      <div class="recording-transcript-surface">',
        '        <div class="recording-transcript-text" id="recording-transcript-rec-1">',
        '          "Buy milk and call Sam."',
        '        <button aria-label="Copy transcript for Morning note" class="recording-copy" title="Copy transcript" type="button">',
        '          <svg aria-hidden="true" fill="none" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.7" stroke="currentColor" viewBox="0 0 24 24">',
        '            <rect height="11" rx="2.4" width="11" x="9" y="9">',
        '            <path d="M6 15H5.4A1.6 1.6 0 0 1 4 13.4V5.4A1.6 1.6 0 0 1 5.4 4h8A1.6 1.6 0 0 1 15 5.4V6">',
        '      <button aria-controls="recording-transcript-rec-1" aria-expanded="false" class="recording-transcript-toggle" hidden="" title="Show full transcript" type="button"> (hidden)',
        '        <span class="recording-transcript-toggle-label">',
        '          "Show more"',
        '        <svg aria-hidden="true" class="recording-transcript-chevron" fill="none" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" stroke="currentColor" viewBox="0 0 24 24">',
        '          <path d="M7.5 10.5 12 15 16.5 10.5">',
        '  <div class="recording-card">',
        '    <button class="recording-row" title="Open raw audio" type="button">',
        '      <span class="recording-row-main">',
        '        "Imported <b>call</b>"',
        '      <span class="recording-row-meta">',
        `        "${when(recordings[1].createdAtMs)} | 1 KB | failed"`,
        '    <button aria-label="Transcribe Imported <b>call</b>" class="recording-transcribe" title="Transcribe Imported <b>call</b>" type="button">',
        '      "Transcribe"',
      ].join("\n"),
    );
  });

  it("shows the empty message when there are no recordings", async () => {
    await openMenuWith([]);

    expect(tree(cards()).join("\n")).toBe(
      [
        '<div class="recent-recordings" id="recent-recordings">',
        '  <div class="recordings-empty">',
        '    "No saved recordings yet"',
      ].join("\n"),
    );
  });

  it("opens the audio, copies the trimmed transcript and starts a transcription", async () => {
    await openMenuWith(recordings);
    const [first, second] = cards().querySelectorAll(".recording-card");

    first.querySelector<HTMLButtonElement>(".recording-row")!.click();
    first.querySelector<HTMLButtonElement>(".recording-copy")!.click();
    second.querySelector<HTMLButtonElement>(".recording-transcribe")!.click();
    await settled();

    expect(invoke).toHaveBeenCalledWith("recording_open", { recordingId: "rec-1" });
    expect(invoke).toHaveBeenCalledWith("copy_to_clipboard", { text: "Buy milk and call Sam." });
    expect(JSON.stringify(invoke.mock.calls)).toContain('"recordingId":"rec-2"');
  });

  it("offers Show more only for a clamped transcript and toggles it", async () => {
    const scrollHeight = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(60);
    await openMenuWith(recordings);
    scrollHeight.mockRestore();
    const panel = cards().querySelector<HTMLElement>(".recording-transcript")!;
    const toggle = panel.querySelector<HTMLButtonElement>(".recording-transcript-toggle")!;
    const label = toggle.querySelector(".recording-transcript-toggle-label")!;

    expect(panel.dataset.clamped).toBe("true");
    expect(toggle.hidden).toBe(false);

    toggle.click();
    expect([panel.dataset.expanded, toggle.getAttribute("aria-expanded"), label.textContent, toggle.title]).toEqual([
      "true",
      "true",
      "Show less",
      "Collapse transcript",
    ]);

    toggle.click();
    expect([panel.dataset.expanded, toggle.getAttribute("aria-expanded"), label.textContent, toggle.title]).toEqual([
      "false",
      "false",
      "Show more",
      "Show full transcript",
    ]);
  });

  it("disables Transcribe while a recording is transcribing", async () => {
    // Saved audio stays transcribing while the app has not answered.
    holdTranscription = true;
    await openMenuWith(recordings);
    cards().querySelector<HTMLButtonElement>(".recording-transcribe")!.click();
    await settled();
    await openMenuWith(recordings);

    expect(cards().querySelector<HTMLButtonElement>(".recording-transcribe")!.disabled).toBe(true);
  });
});
