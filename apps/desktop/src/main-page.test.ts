// @vitest-environment jsdom
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_TRANSCRIPTION_MODEL, LIVE_TRANSCRIPTION_MODEL } from "./models";
import { PCM_FLUSHED } from "./pcm-capture";
import { relaunch } from "@tauri-apps/plugin-process";
import { check } from "@tauri-apps/plugin-updater";

// Renders the real menu page (main.ts) headless with the Tauri bridge faked:
// the recent recording cards it draws, the pill's settings arrow while a
// dictation runs, hover, the Dictionary's target program, model saves and
// failure notices.
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
// The settings the fake app has saved; app_save_settings changes them.
let savedSettings = {
  hasOpenaiApiKey: true,
  openaiApiKeyPreview: "sk-...abcd",
  transcriptionModel: DEFAULT_TRANSCRIPTION_MODEL,
  transcriptionPrompt: "",
  refineEnabled: false,
  refinePrompt: "",
  showIdlePill: true,
  holdToTalk: true,
};
let holdTranscription = false;
// Dictations get a session, so a recording runs until it is stopped.
let dictating = false;

// Set while the final paste waits, so a test can finish it.
let finishPaste: (() => void) | null = null;
let holdPaste = false;

const invoke = vi.hoisted(() => vi.fn());
// The page's event listeners, by event name, so a test can push app events.
const listeners = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, handler);
    return () => {};
  }),
}));
vi.mock("@tauri-apps/plugin-autostart", () => ({ enable: vi.fn(), isEnabled: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn() }));

// The commands the page sent while it loaded.
let startupCommands: string[] = [];

// How the fake app answers each command; a test may override some, and the
// next test starts from this again.
async function answerLikeTheApp(command: string, args?: Record<string, unknown>) {
  if (command === "recording_list") return listed;
  if (command === "app_get_settings") return savedSettings;
  if (command === "app_save_settings") return (savedSettings = { ...savedSettings, ...args });
  if (command === "get_foreground_app") return null;
  if (dictating && command === "recording_begin") return { id: "rec-live" };
  if (dictating && command === "worker_start_session") return { session_id: "session-live" };
  if (holdTranscription && command === "worker_start_session") return new Promise(() => {});
  if (holdPaste && command === "insert_final_text") {
    return new Promise((resolve) => (finishPaste = () => resolve({ outcome: "pasted" })));
  }
  return undefined;
}

const settled = () => new Promise((resolve) => setTimeout(resolve, 20));
const page = () =>
  (
    window as unknown as {
      __openWhisperTest: {
        openMenu: () => Promise<void>;
        collapseOverlay: () => Promise<void>;
        checkForUpdates: () => Promise<void>;
      };
    }
  ).__openWhisperTest;
const cards = () => document.getElementById("recent-recordings")!;
const widget = () => document.getElementById("widget")!;
const menuShown = () => !document.getElementById("mic-menu")!.classList.contains("hidden");
const commandCalls = (command: string) => invoke.mock.calls.filter(([name]) => name === command);
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
  await page().openMenu();
  await settled();
}

// Closes the menu and ends any dictation: a recording stops on the first
// click and its transcription cancels on the second.
async function returnToIdle() {
  await page().collapseOverlay();
  for (let click = 0; click < 2 && widget().dataset.state !== "idle"; click++) {
    widget().click();
    await settled();
  }
  expect([widget().dataset.state, menuShown()]).toEqual(["idle", false]);
}

// Transcribing: a recording stopped by a click on the pill, waiting for its result.
async function startDictation(state: "recording" | "transcribing") {
  document.getElementById("btn-mic")!.click();
  await settled();
  if (state === "transcribing") {
    widget().click();
    await settled();
  }
  expect(widget().dataset.state).toBe(state);
}

beforeAll(async () => {
  // Microphone capture, faked so a recording can start.
  const audioNode = () => ({ connect: vi.fn(), disconnect: vi.fn() });
  class FakeAudioContext {
    state = "suspended";
    sampleRate = 24000;
    audioWorklet = { addModule: async () => {} };
    destination = audioNode();
    resume = async () => void (this.state = "running");
    suspend = async () => void (this.state = "suspended");
    close = async () => void (this.state = "closed");
    createMediaStreamSource = () => ({ ...audioNode(), context: this });
    createAnalyser = () => ({ ...audioNode(), fftSize: 2048, frequencyBinCount: 1024, getByteTimeDomainData() {} });
    createGain = () => ({ ...audioNode(), gain: { value: 1 } });
  }
  class FakeAudioWorkletNode {
    port = {
      onmessage: null as ((event: { data: string }) => void) | null,
      postMessage() {
        this.onmessage?.({ data: PCM_FLUSHED });
      },
    };
    connect = vi.fn();
    disconnect = vi.fn();
  }
  vi.stubGlobal("AudioContext", FakeAudioContext);
  vi.stubGlobal("AudioWorkletNode", FakeAudioWorkletNode);
  Object.defineProperty(navigator, "mediaDevices", {
    value: { getUserMedia: vi.fn(async () => ({ getTracks: () => [] })), enumerateDevices: vi.fn(async () => []) },
  });
  document.body.innerHTML = '<div id="app"></div>';
  invoke.mockImplementation(answerLikeTheApp);
  await import("./main");
  await settled();
  startupCommands = invoke.mock.calls.map(([command]) => command);
});

beforeEach(() => {
  invoke.mockClear();
  invoke.mockImplementation(answerLikeTheApp);
});

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

  it("draws every recording the app returns, in its order, with its full transcript", async () => {
    // The app returns only the newest four (recording_list).
    const newestFour = [4, 3, 2, 1].map((index) => ({
      ...recordings[0],
      id: `rec-${index}`,
      name: `Note ${index}`,
      transcript: `The whole transcript of note ${index}, every word.`,
    }));
    await openMenuWith(newestFour);

    expect([...cards().querySelectorAll(".recording-transcript-text")].map((text) => text.textContent)).toEqual(
      newestFour.map((recording) => recording.transcript),
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

describe("settings arrow on the pill", () => {
  const arrow = () => document.getElementById("btn-dropdown")!;
  const activations = {
    click: () => arrow().click(),
    Enter: () => pressOnArrow("Enter"),
    Space: () => pressOnArrow(" "),
  };

  // A native button clicks itself on Enter or Space unless the key is cancelled.
  function pressOnArrow(key: string) {
    const press = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
    if (arrow().dispatchEvent(press)) arrow().click();
  }

  beforeEach(async () => {
    holdTranscription = false;
    dictating = true;
    await returnToIdle();
  });

  for (const state of ["recording", "transcribing"] as const) {
    for (const [input, activate] of Object.entries(activations)) {
      it(`opens the menu on ${input} while ${state} and lets the dictation run`, async () => {
        await startDictation(state);

        activate();
        await settled();

        expect([widget().dataset.state, menuShown()]).toEqual([state, true]);
      });
    }

    it(`still ends the dictation on a click elsewhere on the pill while ${state}`, async () => {
      await startDictation(state);

      widget().click();
      await settled();

      expect([widget().dataset.state, menuShown()]).toEqual([state === "recording" ? "transcribing" : "idle", false]);
    });

    it(`closes the open menu and ends the dictation on Stop while ${state}`, async () => {
      await startDictation(state);
      arrow().click();
      await settled();

      document.getElementById("btn-stop")!.click();
      await settled();

      expect([widget().dataset.state, menuShown()]).toEqual([state === "recording" ? "transcribing" : "idle", false]);
    });

    it(`closes the open menu from the arrow and lets the dictation run while ${state}`, async () => {
      await startDictation(state);
      arrow().click();
      await settled();

      arrow().click();
      await settled();

      expect([widget().dataset.state, menuShown()]).toEqual([state, false]);
    });
  }

  it("enables Transcribe again when a dictation ends after its list redrew during the paste", async () => {
    await startDictation("recording");
    listed = recordings;
    arrow().click();
    await settled();
    // Enter on the pill stops the recording and keeps the menu open.
    widget().dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    await settled();
    holdPaste = true;
    listeners.get("worker-session-event")!({
      payload: { session_id: "session-live", event: { type: "final", text: "Hello" }, done: true },
    });
    await settled();
    const transcribe = () => cards().querySelector<HTMLButtonElement>(".recording-transcribe")!;
    expect([widget().dataset.state, transcribe().disabled]).toEqual(["transcribing", true]);

    finishPaste!();
    holdPaste = false;
    await settled();

    expect([widget().dataset.state, menuShown(), transcribe().disabled]).toEqual(["idle", true, false]);
  });
});

describe("pill and menu behaviour once guarded by source text checks", () => {
  beforeEach(async () => {
    holdTranscription = false;
    dictating = true;
    await returnToIdle();
  });

  it("expands and collapses the pill on hover without resizing the native window", async () => {
    // Hover only changes the CSS pill inside the window. A window layout that
    // read hover state would resize and move the window under the cursor, so
    // the pill would jump and flicker between hover and leave.
    // Collapsing above sent the window its layout; only hover follows.
    expect(widget().dataset.expanded).toBe("false");
    invoke.mockClear();

    widget().dispatchEvent(new PointerEvent("pointerenter"));
    await vi.waitFor(() => expect(widget().dataset.expanded).toBe("true"));
    widget().dispatchEvent(new PointerEvent("pointerleave"));
    await vi.waitFor(() => expect(widget().dataset.expanded).toBe("false"));
    expect(commandCalls("overlay_apply_layout")).toEqual([]);

    // The spy sees layouts: opening the menu does change the window.
    await page().openMenu();
    expect(commandCalls("overlay_apply_layout").length).toBeGreaterThan(0);
  });

  it("asks for the Dictionary's target program each time the menu opens", async () => {
    // F1 (2026-11-23): Rust no longer sends every app switch to the menu. The
    // menu asks on open and shows "Detecting…" meanwhile, so a reopened menu
    // never shows the previous program's name.
    const answers: ((name: string) => void)[] = [];
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) =>
      command === "get_foreground_app"
        ? new Promise((resolve) => answers.push(resolve))
        : answerLikeTheApp(command, args),
    );
    const targetProgram = () => document.getElementById("menu-target-app-name")!.textContent;

    expect(startupCommands).not.toContain("get_foreground_app");
    expect(listeners.has("foreground-app-changed")).toBe(false);
    for (const program of ["notepad", "Code"]) {
      await page().openMenu();
      expect(targetProgram()).toBe("Detecting…");
      answers.at(-1)!(program);
      await vi.waitFor(() => expect(targetProgram()).toBe(program));
      await page().collapseOverlay();
    }
    expect(commandCalls("get_foreground_app")).toHaveLength(2);
  });

  it("starts a recording with the model the app is saving, not the one the page still shows", async () => {
    // Bug (F3, 2026-11-21): Live, Standard, Live quickly left the page on Live
    // while the app had saved Standard, so a recording streamed Live audio to a
    // Standard session. The page keeps its older settings while the model
    // saves, so a recording asks the app; Rust reads the model it saved.
    let finishSave!: () => void;
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command !== "app_save_settings") return answerLikeTheApp(command, args);
      const saved = answerLikeTheApp(command, args);
      return new Promise((resolve) => (finishSave = () => resolve(saved)));
    });
    const modelSelect = document.getElementById("menu-model-select") as HTMLSelectElement;
    expect(modelSelect.value).toBe(DEFAULT_TRANSCRIPTION_MODEL);

    modelSelect.value = LIVE_TRANSCRIPTION_MODEL;
    modelSelect.dispatchEvent(new Event("change"));
    await vi.waitFor(() => expect(finishSave).toBeDefined());
    listeners.get("toggle-recording")!({ payload: undefined });
    await vi.waitFor(() => expect(commandCalls("worker_start_session")).toHaveLength(1));

    expect(commandCalls("app_get_settings")).toHaveLength(1);
    expect(commandCalls("worker_start_session")[0][1]).toEqual({});

    // Puts Standard back for the tests after this one.
    finishSave();
    await returnToIdle();
    invoke.mockImplementation(answerLikeTheApp);
    modelSelect.value = DEFAULT_TRANSCRIPTION_MODEL;
    modelSelect.dispatchEvent(new Event("change"));
    await vi.waitFor(() => expect(savedSettings.transcriptionModel).toBe(DEFAULT_TRANSCRIPTION_MODEL));
  });

  it("shows a failure as a notice on the pill without opening the menu", async () => {
    // Bug (fixed in 38cabe0): every failure forced the menu open over the user's work.
    await startDictation("recording");

    listeners.get("worker-session-event")!({
      payload: { session_id: "session-live", event: { type: "error", text: "Network down" }, done: true },
    });
    await vi.waitFor(() => expect(widget().dataset.notice).toBe("true"));

    expect([
      widget().dataset.state,
      document.getElementById("widget-notice-text")!.textContent,
      widget().title,
      menuShown(),
    ]).toEqual(["idle", "Network down", "Network down", false]);
  });
});

describe("Windows update", () => {
  // What the update did, in order, with the pill's state when it installed.
  let steps: string[] = [];
  let installFails = false;
  // Settles when the fake installer finishes; a test may hold it.
  let installing: Promise<void> = Promise.resolve();
  const update = {
    version: "9.9.9",
    download: vi.fn(async () => void steps.push("download")),
    install: vi.fn(async () => {
      steps.push(`install while ${widget().dataset.state}`);
      await installing;
      if (installFails) throw new Error("installer failed");
    }),
    downloadAndInstall: vi.fn(async () => {
      await update.download();
      await update.install();
    }),
  };
  const toastText = () => document.getElementById("toast")!.textContent;

  beforeEach(async () => {
    holdTranscription = false;
    dictating = true;
    await returnToIdle();
    steps = [];
    installFails = false;
    installing = Promise.resolve();
    vi.mocked(check).mockResolvedValue(update as never);
    vi.mocked(relaunch).mockImplementation(async () => void steps.push("relaunch"));
  });

  it("installs an accepted update at once and relaunches when no dictation runs", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(true);

    await page().checkForUpdates();

    expect(steps).toEqual(["download", "install while idle", "relaunch"]);
  });

  it("downloads, installs and relaunches nothing when the user declines", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(false);

    await page().checkForUpdates();

    expect(steps).toEqual([]);
  });

  it("keeps the current app and says so when an accepted update fails", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(true);
    installFails = true;

    await page().checkForUpdates();

    expect([steps, toastText()]).toEqual([
      ["download", "install while idle"],
      "Update failed. Your current app and settings are unchanged.",
    ]);
    // Dictations start again once the install has failed.
    await startDictation("recording");
  });

  it("installs only once a running dictation has pasted its text, then starts no new one", async () => {
    // F1 (2026-11-28): the installer closes the app, so installing during a
    // dictation lost the recording's end and its transcript.
    vi.spyOn(window, "confirm").mockReturnValue(true);
    await startDictation("recording");

    const checking = page().checkForUpdates();
    await settled();
    expect([steps, toastText()]).toEqual([["download"], "The update will install when this dictation finishes."]);

    widget().click();
    await settled();
    holdPaste = true;
    listeners.get("worker-session-event")!({
      payload: { session_id: "session-live", event: { type: "final", text: "Hello" }, done: true },
    });
    await settled();
    expect([steps, widget().dataset.state]).toEqual([["download"], "transcribing"]);

    let finishInstall!: () => void;
    installing = new Promise((resolve) => (finishInstall = resolve));
    finishPaste!();
    holdPaste = false;
    await settled();
    expect(steps).toEqual(["download", "install while idle"]);

    // The pill, its microphone button and both shortcuts all start nothing now.
    invoke.mockClear();
    widget().click();
    document.getElementById("btn-mic")!.click();
    listeners.get("toggle-recording")!({ payload: undefined });
    listeners.get("press-to-talk-start")!({ payload: undefined });
    listeners.get("press-to-talk-stop")!({ payload: undefined });
    await settled();
    expect([widget().dataset.state, toastText(), commandCalls("recording_begin")]).toEqual([
      "idle",
      "Installing the OpenWhisper update…",
      [],
    ]);

    finishInstall();
    await checking;
    expect(steps).toEqual(["download", "install while idle", "relaunch"]);
  });
});
