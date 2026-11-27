import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  enable as enableAutostart,
  isEnabled as isAutostartEnabled,
} from "@tauri-apps/plugin-autostart";
import { relaunch } from "@tauri-apps/plugin-process";
import { check } from "@tauri-apps/plugin-updater";
import {
  COLLAPSED_HEIGHT,
  COLLAPSED_WIDTH,
  EXPANDED_HEIGHT,
  EXPANDED_WIDTH,
  createWindowLayoutQueue,
  getOverlayLayout,
  isOverlayVisible,
} from "./overlay-layout";
import {
  DEFAULT_TRANSCRIPTION_MODEL,
  REFINE_MODEL,
  transcriptionModelHint,
  transcriptionModelOptionsHtml,
  type TranscriptionModel,
} from "./models";
import { createMicrophoneCapture } from "./microphone-capture";
import {
  createDictationController,
  type WidgetState,
  type WorkerSessionEvent,
} from "./dictation-controller";
import { errorMessage } from "./error-message";
import { createSettingsSaver } from "./settings-save";
import { createRecordingListRefresh } from "./recording-list-refresh";
import { METER_SLICES, startMeter } from "./meter";
import "./styles.css";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------
type AppSettings = {
  hasOpenaiApiKey: boolean;
  openaiApiKeyPreview: string | null;
  transcriptionModel: TranscriptionModel;
  transcriptionPrompt: string;
  refineEnabled: boolean;
  refinePrompt: string;
  showIdlePill: boolean;
  holdToTalk: boolean;
};
type MicDevice = { id: string; label: string };
type RecordingMetadata = {
  id: string;
  createdAtMs: number;
  name: string;
  source: string;
  mimeType: string;
  fileName: string;
  status: string;
  bytes: number;
  transcript: string;
  error: string;
};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------
// The settings the menu shows. Every change passes through applySettingsUi,
// so a recording reads them here instead of asking the app each time.
let appSettings: AppSettings | null = null;
// Stops the recording meter; null when none is running.
let meterStop: (() => void) | null = null;
let micDevices: MicDevice[] = [];
let selectedMicId = "default";
let menuOpen = false;

const HOVER_EXPAND_DELAY_MS = 40;
const HOVER_COLLAPSE_GRACE_MS = 90;
const NOTICE_DURATION_MS = 5000;
// Left inset + status dot + gaps + dropdown button around the notice text.
const NOTICE_CHROME_WIDTH = 52;
const NOTICE_MAX_WIDTH = 180;

// ---------------------------------------------------------------------------
// DOM
// ---------------------------------------------------------------------------
const app = document.getElementById("app")!;
app.innerHTML = `
  <div id="widget-shell">
    <div id="api-key-bubble" class="api-key-bubble hidden" role="button" tabindex="0" aria-label="Add your OpenAI API key in settings">
      <span class="api-key-bubble-logo" aria-hidden="true">
        <svg viewBox="0 0 24 24" fill="none">
          <path d="M2 4L6 20L12 10L18 20L22 4" />
        </svg>
      </span>
      <span class="api-key-bubble-text">Add OpenAI API key</span>
      <span class="api-key-bubble-arrow" aria-hidden="true">
        <svg viewBox="0 0 20 20" fill="none">
          <path d="M7 5L12 10L7 15" />
        </svg>
      </span>
    </div>
    <div id="widget" tabindex="0" role="button" aria-label="Click to start or stop transcription">
      <div class="widget-surface" aria-hidden="true"></div>

      <span class="meter" aria-hidden="true">
        <span class="meter-baseline" aria-hidden="true"></span>
        <svg class="meter-idle-wave" viewBox="0 0 76 28" aria-hidden="true">
          <path class="meter-idle-wave-path" d="M0,14 C6,11 12,17 18,14 C24,11 30,17 36,14 C42,11 48,17 54,14 C60,11 66,17 72,14 C74,12 76,14" fill="none" stroke="currentColor" stroke-width="0.9" stroke-linecap="round" />
        </svg>
        <canvas id="meter-canvas" class="meter-canvas" width="48" height="28"></canvas>
      </span>

      <button id="btn-stop" class="stop-btn" type="button" title="Stop recording" aria-hidden="true">
        <svg viewBox="0 0 24 24" fill="currentColor">
          <rect x="6.5" y="6.5" width="11" height="11" rx="3" />
        </svg>
      </button>

      <button id="btn-mic" class="mic-btn" type="button" title="Click to start recording">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z" />
          <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
          <line x1="12" x2="12" y1="19" y2="22" />
        </svg>
      </button>

      <span id="record-control" class="record-control" aria-hidden="true" title="Click to toggle recording">
        <svg class="icon-dots" viewBox="0 0 24 8" fill="currentColor">
          <circle cx="4" cy="4" r="2" />
          <circle cx="12" cy="4" r="2" />
          <circle cx="20" cy="4" r="2" />
        </svg>
        <svg class="icon-spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4">
          <path d="M12 2.75A9.25 9.25 0 0 1 21.25 12" stroke-linecap="round" />
        </svg>
      </span>

      <span id="widget-notice" class="widget-notice" role="status" aria-live="polite">
        <span class="widget-notice-dot" aria-hidden="true"></span>
        <span id="widget-notice-text" class="widget-notice-text"></span>
      </span>

      <button id="btn-dropdown" class="dropdown-btn" type="button" title="Select microphone and settings">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">
          <path d="M7.5 10.5 12 15 16.5 10.5" />
        </svg>
      </button>
    </div>

    <div id="mic-menu" class="mic-menu hidden">
      <div id="failure-panel" class="failure-panel hidden" role="alert">
        <div class="failure-title">Transcription failed</div>
        <div id="failure-message" class="failure-message"></div>
        <div class="menu-action-row">
          <button id="failure-open-recording" class="menu-action" type="button">Open raw audio</button>
          <button id="failure-dismiss" class="menu-action menu-action-secondary" type="button">Dismiss</button>
        </div>
      </div>
      <div class="menu-section">Recordings &amp; transcripts</div>
      <div class="menu-action-row menu-action-row-wide">
        <button id="menu-import-audio" class="menu-action" type="button">Transcribe audio file</button>
        <button id="menu-open-recordings" class="menu-icon-action" type="button" title="Open recordings folder" aria-label="Open recordings folder">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
            <path d="M3.5 7.5h6l2 2h9v9.5a1.5 1.5 0 0 1-1.5 1.5H5A1.5 1.5 0 0 1 3.5 19Z" />
            <path d="M3.5 7.5V5A1.5 1.5 0 0 1 5 3.5h5l2 2h5" />
          </svg>
        </button>
        <button id="menu-open-logs" class="menu-icon-action" type="button" title="Open log folder" aria-label="Open log folder">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true">
            <path d="M6.5 3.5h8l4 4v11.5a1.5 1.5 0 0 1-1.5 1.5H6.5A1.5 1.5 0 0 1 5 19V5a1.5 1.5 0 0 1 1.5-1.5Z" />
            <path d="M8.5 11h7M8.5 14h7M8.5 17h4.5" />
          </svg>
        </button>
      </div>
      <div id="recent-recordings" class="recent-recordings"></div>
      <template id="recording-card-template">
        <div class="recording-card">
          <button class="recording-row" type="button" title="Open raw audio">
            <span class="recording-row-main"></span>
            <span class="recording-row-meta"></span>
          </button>
          <div class="recording-transcript" data-expanded="false" data-clamped="false">
            <div class="recording-transcript-surface">
              <div class="recording-transcript-text"></div>
              <button class="recording-copy" type="button" title="Copy transcript">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                  <rect x="9" y="9" width="11" height="11" rx="2.4" />
                  <path d="M6 15H5.4A1.6 1.6 0 0 1 4 13.4V5.4A1.6 1.6 0 0 1 5.4 4h8A1.6 1.6 0 0 1 15 5.4V6" />
                </svg>
              </button>
            </div>
            <button class="recording-transcript-toggle" type="button" aria-expanded="false" title="Show full transcript" hidden>
              <span class="recording-transcript-toggle-label">Show more</span>
              <svg class="recording-transcript-chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M7.5 10.5 12 15 16.5 10.5" /></svg>
            </button>
          </div>
          <button class="recording-transcribe" type="button">Transcribe</button>
        </div>
      </template>
      <div class="menu-divider"></div>
      <div class="menu-section">Appearance</div>
      <div class="menu-toggle-row">
        <span class="menu-section menu-section-inline">Show pill when idle</span>
        <label class="toggle-switch" title="Keep the collapsed pill visible when not recording">
          <input id="menu-show-idle-pill-toggle" type="checkbox" aria-label="Show pill when idle" />
          <span class="toggle-track"></span>
          <span class="toggle-thumb"></span>
        </label>
      </div>
      <div class="menu-prompt-hint">Turn this off to completely hide the collapsed pill. Use the tray icon to show it again.</div>
      <div class="menu-divider"></div>
      <div class="menu-section">Shortcut</div>
      <div class="menu-toggle-row">
        <span class="menu-section menu-section-inline">Hold Ctrl+Space to talk</span>
        <label class="toggle-switch" title="When off, press Ctrl+Space once to start and again to finish">
          <input id="menu-hold-to-talk-toggle" type="checkbox" aria-label="Hold Ctrl+Space to talk" />
          <span class="toggle-track"></span>
          <span class="toggle-thumb"></span>
        </label>
      </div>
      <div class="menu-prompt-hint">Turn this off to press Ctrl+Space once to start listening and again to finish.</div>
      <div class="menu-divider"></div>
      <div class="menu-section">Microphone</div>
      <div id="mic-list"></div>
      <div class="menu-divider"></div>
      <div class="menu-section">Transcription model</div>
      <select id="menu-model-select" class="menu-select" aria-label="Transcription model">
        ${transcriptionModelOptionsHtml()}
      </select>
      <div id="menu-model-hint" class="menu-prompt-hint"></div>
      <div class="menu-divider"></div>
      <div class="menu-section">OpenAI API Key</div>
      <div class="menu-item" id="menu-key-status">Not configured</div>
      <input id="menu-key-input" class="menu-key-input" type="password" placeholder="sk-..." autocomplete="off" spellcheck="false" />
      <a id="menu-key-link" class="menu-link" href="https://platform.openai.com/api-keys">
        <span class="menu-link-bold">Create or manage</span> keys at platform.openai.com/api-keys
      </a>
      <div class="menu-divider"></div>
      <div class="menu-section">Dictionary</div>
      <div id="menu-target-app" class="menu-target-app">
        <span class="target-app-dot"></span>
        <span id="menu-target-app-name">Detecting…</span>
      </div>
      <textarea id="menu-prompt-input" class="menu-prompt-input" rows="3" placeholder="e.g. Glossary: @README.md, @package.json, Daniël" spellcheck="false"></textarea>
      <div class="menu-prompt-hint">Custom vocabulary hints for the transcription model (224 token limit)</div>
      <div class="menu-divider"></div>
      <div class="menu-toggle-row">
        <span class="menu-section menu-section-inline">Refine with AI</span>
        <label class="toggle-switch">
          <input id="menu-refine-toggle" type="checkbox" />
          <span class="toggle-track"></span>
          <span class="toggle-thumb"></span>
        </label>
      </div>
      <textarea id="menu-refine-prompt-input" class="menu-prompt-input" rows="3" placeholder="Leave blank for default: strip fillers, fix sentences, add lists when enumerated" spellcheck="false"></textarea>
      <div class="menu-prompt-hint">Custom instructions for the refinement pass (${REFINE_MODEL}). Short, clean dictations are pasted without it; longer ones can take about 1-2 seconds more.</div>
    </div>
  </div>

  <div id="toast" class="toast hidden"></div>
`;

const apiKeyBubble = document.getElementById("api-key-bubble")!;
const widget = document.getElementById("widget")!;
const meterCanvas = document.getElementById("meter-canvas") as HTMLCanvasElement;
const btnMic = document.getElementById("btn-mic") as HTMLButtonElement;
const btnDropdown = document.getElementById("btn-dropdown") as HTMLButtonElement;
const micMenu = document.getElementById("mic-menu")!;
const micList = document.getElementById("mic-list")!;
const menuModelSelect = document.getElementById("menu-model-select") as HTMLSelectElement;
const menuModelHint = document.getElementById("menu-model-hint")!;
const toast = document.getElementById("toast")!;
const menuKeyStatus = document.getElementById("menu-key-status")!;
const menuKeyInput = document.getElementById("menu-key-input") as HTMLInputElement;
const menuKeyLink = document.getElementById("menu-key-link") as HTMLAnchorElement;
const menuPromptInput = document.getElementById("menu-prompt-input") as HTMLTextAreaElement;
const menuRefineToggle = document.getElementById("menu-refine-toggle") as HTMLInputElement;
const menuRefinePromptInput = document.getElementById("menu-refine-prompt-input") as HTMLTextAreaElement;
const menuShowIdlePillToggle = document.getElementById("menu-show-idle-pill-toggle") as HTMLInputElement;
const menuHoldToTalkToggle = document.getElementById("menu-hold-to-talk-toggle") as HTMLInputElement;
const menuTargetAppName = document.getElementById("menu-target-app-name")!;
const failurePanel = document.getElementById("failure-panel")!;
const failureMessage = document.getElementById("failure-message")!;
const failureOpenRecording = document.getElementById("failure-open-recording") as HTMLButtonElement;
const failureDismiss = document.getElementById("failure-dismiss") as HTMLButtonElement;
const menuImportAudio = document.getElementById("menu-import-audio") as HTMLButtonElement;
const menuOpenRecordings = document.getElementById("menu-open-recordings") as HTMLButtonElement;
const menuOpenLogs = document.getElementById("menu-open-logs") as HTMLButtonElement;
const recentRecordings = document.getElementById("recent-recordings")!;
const recordingCardTemplate = document.getElementById("recording-card-template") as HTMLTemplateElement;
const widgetNoticeText = document.getElementById("widget-notice-text")!;

let hasOpenaiApiKey = false;
let settingsLoaded = false;
let storedKeyPreview = "";
let showingStoredKeyPreview = false;
let keyInputDirty = false;
let keyRevealInFlight = false;
let promptDirty = false;
let refineDirty = false;
let showIdlePill = true;
let manualOverlayReveal = false;
let widgetHovered = false;
let expandTimeoutId: number | null = null;
let collapseTimeoutId: number | null = null;
let noticeWidth = 0;
let noticeTimeoutId: number | null = null;
const windowLayoutQueue = createWindowLayoutQueue((layout) =>
  invoke<void>("overlay_apply_layout", {
    width: layout.width,
    height: layout.height,
    visible: layout.visible,
  }),
);

// ---------------------------------------------------------------------------
// Dictation: dictation-controller.ts runs the flow and holds its state; the
// page gives it the microphone and shows what it does.
// ---------------------------------------------------------------------------
const microphoneCapture = createMicrophoneCapture({
  selectedMicrophoneId: () => selectedMicId,
  onFellBackToDefaultMicrophone() {
    selectedMicId = "default";
    renderMicList();
  },
  log: logDebug,
});

const dictation = createDictationController({
  invoke,
  capture: microphoneCapture,
  settings: currentSettings,
  apiKeyKnownMissing: () => appSettings?.hasOpenaiApiKey === false,
  setState,
  showToast,
  showFailure,
  clearNotice: clearWidgetNotice,
  startMeter() {
    widget.dataset.silent = "true";
    meterStop?.();
    const source = microphoneCapture.source();
    meterStop = source && startMeter(source, {
      draw: drawMeter,
      isRecording: () => dictation.state === "recording",
      setSilent: (silent) => {
        widget.dataset.silent = String(silent);
      },
    });
  },
  stopMeter() {
    if (meterStop) {
      meterStop();
      meterStop = null;
      delete widget.dataset.silent;
    }
  },
  showTranscript(text, final) {
    widget.dataset.liveTranscript = text;
    widget.setAttribute("aria-label", `${final ? "Transcription" : "Live transcription"}: ${text}`);
  },
  clearTranscript() {
    widget.removeAttribute("data-live-transcript");
    widget.setAttribute("aria-label", "Click to start or stop transcription");
  },
  recordingsChanged: refreshRecordings,
  closeMenu: () => void collapseOverlay(),
  log: logDebug,
});

// The app pushes progress, live text and the result of each session.
void listen<WorkerSessionEvent>("worker-session-event", ({ payload }) => {
  void dictation.receiveSessionEvent(payload);
});

// Native end-to-end smoke hook for development builds (runRealtimeSmoke in
// dictation-controller.ts): real commands and events, deterministic audio.
if (import.meta.env.DEV) {
  (window as typeof window & {
    __openwhisperRealtimeSmoke?: (chunks: string[], intervalMs?: number) => Promise<void>;
  }).__openwhisperRealtimeSmoke = dictation.runRealtimeSmoke;
}

function logDebug(message: string) {
  const timestamp = new Date().toISOString();
  void invoke("debug_log", { message: `${timestamp} ${message}` }).catch(() => {});
}

function isWidgetExpanded() {
  return widgetHovered || menuOpen || dictation.state !== "idle" || noticeWidth > 0;
}

function getWidgetSize() {
  if (noticeWidth > 0) {
    return { width: noticeWidth, height: EXPANDED_HEIGHT };
  }
  if (isWidgetExpanded()) {
    return { width: EXPANDED_WIDTH, height: EXPANDED_HEIGHT };
  }
  return { width: COLLAPSED_WIDTH, height: COLLAPSED_HEIGHT };
}

const widgetShell = document.getElementById("widget-shell")!;

function syncWidgetFrame() {
  const widgetSize = getWidgetSize();
  widgetShell.style.setProperty("--widget-width", `${widgetSize.width}px`);
  widgetShell.style.setProperty("--widget-height", `${widgetSize.height}px`);
  widget.dataset.expanded = String(isWidgetExpanded());
  widget.dataset.hovered = String(widgetHovered);
  widget.dataset.menuOpen = String(menuOpen);
  widget.dataset.notice = String(noticeWidth > 0);
}

function syncApiKeyBubble() {
  const show = settingsLoaded && !hasOpenaiApiKey && dictation.state === "idle";
  apiKeyBubble.classList.toggle("hidden", !show);
}

function setWidgetHovered(next: boolean) {
  if (widgetHovered === next) return;
  widgetHovered = next;
  widget.dataset.hovered = String(next);
  void applyOverlayLayout();
}

function getWindowLayout() {
  const menuVisible = menuOpen && !micMenu.classList.contains("hidden");
  const menuRect = menuVisible ? micMenu.getBoundingClientRect() : undefined;
  const layout = getOverlayLayout({
    noticeWidth,
    settingsLoaded,
    hasOpenaiApiKey: hasOpenaiApiKey || dictation.state !== "idle",
    menuVisible,
    menuWidth: menuRect?.width,
    menuHeight: menuRect?.height,
  });
  return {
    ...layout,
    visible: isOverlayVisible({
      showIdlePill,
      idle: dictation.state === "idle",
      menuOpen,
      manualReveal: manualOverlayReveal,
      noticeShowing: noticeWidth > 0,
    }),
  };
}

async function applyOverlayLayout(force = false) {
  syncWidgetFrame();
  await windowLayoutQueue.apply(getWindowLayout(), force);
}

// ---------------------------------------------------------------------------
// Mic devices
// ---------------------------------------------------------------------------
async function refreshMicDevices() {
  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    stream.getTracks().forEach((t) => t.stop());
  } catch {
    // permission denied — will show empty list
  }
  const devices = await navigator.mediaDevices.enumerateDevices();
  micDevices = devices
    .filter((d) => d.kind === "audioinput")
    .map((d, i) => ({ id: d.deviceId, label: d.label || `Microphone ${i + 1}` }));
  renderMicList();
}

function renderMicList() {
  micList.innerHTML = "";
  const all: MicDevice[] = [{ id: "default", label: "System default" }, ...micDevices];
  for (const dev of all) {
    const el = document.createElement("div");
    el.className = `menu-item${dev.id === selectedMicId ? " selected" : ""}`;
    el.textContent = dev.label;
    el.addEventListener("click", () => {
      selectedMicId = dev.id;
      renderMicList();
      void collapseOverlay();
    });
    micList.appendChild(el);
  }
}

// ---------------------------------------------------------------------------
// Menu
// ---------------------------------------------------------------------------
function toggleMenu() {
  void (micMenu.classList.contains("hidden") ? openMenu() : collapseOverlay());
}

async function openMenu() {
  menuOpen = true;
  micMenu.classList.remove("hidden");
  void refreshForegroundApp();
  // The cards were not refreshed while the menu was closed, so a recording
  // transcribed meanwhile can still show Transcribe; the new list redraws them.
  recentRecordings
    .querySelectorAll<HTMLButtonElement>(".recording-transcribe")
    .forEach((button) => (button.disabled = true));
  refreshRecordings();
  await applyOverlayLayout();
}

async function collapseOverlay() {
  menuOpen = false;
  clearHoverTimers();
  setWidgetHovered(false);
  micMenu.classList.add("hidden");
  // A failure panel only lingers while its widget notice is still showing.
  if (noticeWidth === 0) failurePanel.classList.add("hidden");
  await applyOverlayLayout();
}

btnDropdown.addEventListener("click", (e) => {
  e.stopPropagation();
  toggleMenu();
});

async function openSettingsForApiKey() {
  if (!menuOpen) {
    await openMenu();
  }
  requestAnimationFrame(() => {
    menuKeyInput.focus();
  });
}

apiKeyBubble.addEventListener("click", (e) => {
  e.stopPropagation();
  e.preventDefault();
  void openSettingsForApiKey();
});

apiKeyBubble.addEventListener("keydown", (e) => {
  if (e.key === "Enter" || e.key === " ") {
    e.preventDefault();
    void openSettingsForApiKey();
  }
});

/** Stops a recording or cancels a transcription; false when neither is running. */
function stopOrCancel(): boolean {
  if (dictation.state === "recording") void dictation.stopRecording();
  else if (dictation.state === "transcribing") void dictation.cancelTranscription();
  else return false;
  return true;
}

// The settings arrow always reaches its own click listener, which opens or
// closes the menu, so it never stops a recording or cancels a transcription.
// Any other click on the pill closes an open menu and still ends a dictation.
function handleWidgetClick(e: MouseEvent) {
  if (btnDropdown.contains(e.target as Node)) return;
  if (menuOpen) {
    void collapseOverlay();
    stopOrCancel();
  } else if (!stopOrCancel()) {
    if (btnMic.contains(e.target as Node)) return;
    void dictation.toggleRecording();
    return;
  }
  e.stopPropagation();
  e.preventDefault();
}

function handleWidgetKeydown(e: KeyboardEvent) {
  if (e.key !== "Enter" && e.key !== " ") return;
  if (btnDropdown.contains(e.target as Node)) return;
  if (stopOrCancel()) {
    e.preventDefault();
    return;
  }
  if (btnMic.contains(e.target as Node)) return;
  e.preventDefault();
  void dictation.toggleRecording();
}

widget.addEventListener("click", handleWidgetClick, true);

widget.addEventListener("keydown", handleWidgetKeydown);

function clearHoverTimers() {
  if (expandTimeoutId !== null) clearTimeout(expandTimeoutId);
  if (collapseTimeoutId !== null) clearTimeout(collapseTimeoutId);
  expandTimeoutId = null;
  collapseTimeoutId = null;
}

function handleWidgetPointerEnter() {
  if (collapseTimeoutId !== null) {
    clearTimeout(collapseTimeoutId);
    collapseTimeoutId = null;
  }
  if (expandTimeoutId !== null || widgetHovered) return;
  expandTimeoutId = window.setTimeout(() => {
    expandTimeoutId = null;
    setWidgetHovered(true);
  }, HOVER_EXPAND_DELAY_MS);
}

function handleWidgetPointerLeave(event: PointerEvent) {
  if (event.relatedTarget instanceof Node && widget.contains(event.relatedTarget)) {
    return;
  }
  clearHoverTimers();
  if (!widgetHovered) return;
  collapseTimeoutId = window.setTimeout(() => {
    collapseTimeoutId = null;
    setWidgetHovered(false);
  }, HOVER_COLLAPSE_GRACE_MS);
}

widget.addEventListener("pointerenter", handleWidgetPointerEnter);
widget.addEventListener("pointerleave", handleWidgetPointerLeave);

document.addEventListener("click", (e) => {
  if (!micMenu.contains(e.target as Node) && !micMenu.classList.contains("hidden")) {
    void collapseOverlay();
  }
});

window.addEventListener("blur", () => {
  if (menuOpen) void collapseOverlay();
});

// ---------------------------------------------------------------------------
// Toast
// ---------------------------------------------------------------------------
let toastTimer: number | null = null;
function showToast(msg: string, durationMs = 3000) {
  toast.textContent = msg;
  toast.classList.remove("hidden");
  if (toastTimer) clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => toast.classList.add("hidden"), durationMs);
}

// ---------------------------------------------------------------------------
// Visual state
// ---------------------------------------------------------------------------
function setState(next: WidgetState) {
  // Starting looks like a recording that has not heard sound yet, so the pill
  // does not change when the microphone comes on.
  widget.dataset.state = next === "starting" ? "recording" : next;
  if (next === "starting") widget.dataset.silent = "true";
  else if (next === "idle") {
    delete widget.dataset.silent;
    // An open menu's Transcribe buttons were drawn disabled while busy.
    refreshRecordings();
  }
  syncApiKeyBubble();
  void applyOverlayLayout();
}

// ---------------------------------------------------------------------------
// Meter
// ---------------------------------------------------------------------------
// The scrolling waveform while recording; new bars enter on the right.
const METER_BAR_WIDTH = 3;
const METER_COLOR = "#9ef0c9";

function drawMeter(history: readonly number[]) {
  const canvas = meterCanvas;
  const context = canvas.getContext("2d");
  if (!context) return;
  const { width, height } = canvas;
  context.fillStyle = METER_COLOR;
  context.clearRect(0, 0, width, height);
  context.beginPath();
  const sliceWidth = width / METER_SLICES;
  const firstSlice = METER_SLICES - history.length;
  history.forEach((value, index) => {
    let fraction = value / 255;
    if (fraction < 0.05) fraction += 0.05;
    const top = (height / 2) * fraction;
    context.rect((firstSlice + index) * sliceWidth, top, METER_BAR_WIDTH, height - 2 * top);
  });
  context.fill();
}

// ---------------------------------------------------------------------------
// Failures
// ---------------------------------------------------------------------------
function showFailure(message: string, recordingSaved: boolean) {
  const sentence = /[.!?]$/.test(message) ? message : `${message}.`;
  failureMessage.textContent = recordingSaved
    ? `${sentence} The raw audio was saved locally and can be opened below.`
    : message;
  failureOpenRecording.disabled = !dictation.lastFailedRecordingId;
  failurePanel.classList.remove("hidden");
  // Failures never open the menu or steal the flow: the pill briefly shows a
  // short notice in place, and the menu (with the raw audio) stays one click
  // away on its dropdown until the notice fades.
  showWidgetNotice(message.length <= 24 ? message : "Transcription failed", message);
}

function showWidgetNotice(text: string, detail: string) {
  widgetNoticeText.textContent = text;
  widget.title = detail;
  // scrollWidth is the text's natural width even while the collapsed pill
  // squeezes the clipped label.
  noticeWidth = Math.min(
    NOTICE_MAX_WIDTH,
    Math.max(EXPANDED_WIDTH, widgetNoticeText.scrollWidth + NOTICE_CHROME_WIDTH),
  );
  if (noticeTimeoutId !== null) clearTimeout(noticeTimeoutId);
  noticeTimeoutId = window.setTimeout(() => clearWidgetNotice(), NOTICE_DURATION_MS);
  void applyOverlayLayout(true);
}

function clearWidgetNotice() {
  if (noticeTimeoutId !== null) {
    clearTimeout(noticeTimeoutId);
    noticeTimeoutId = null;
  }
  if (noticeWidth === 0) return;
  noticeWidth = 0;
  widget.removeAttribute("title");
  if (!menuOpen) failurePanel.classList.add("hidden");
  void applyOverlayLayout(true);
}

function formatRecordingSize(bytes: number): string {
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

const refreshRecordingList = createRecordingListRefresh({
  isMenuOpen: () => menuOpen,
  list: () => invoke<RecordingMetadata[]>("recording_list"),
  draw: drawRecordings,
});

function refreshRecordings() {
  refreshRecordingList().catch((error) => {
    logDebug(`recording list failed=${errorMessage(error)}`);
  });
}

async function drawRecordings(recordings: RecordingMetadata[]) {
  recentRecordings.replaceChildren();
  for (const recording of recordings.slice(0, 4)) {
    const transcript = recording.transcript.trim();
    const card = recordingCardTemplate.content.firstElementChild!.cloneNode(true) as HTMLElement;
    const cardPart = <PartElement extends HTMLElement>(selector: string) =>
      card.querySelector<PartElement>(selector)!;
    const when = new Date(recording.createdAtMs).toLocaleString([], {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
    cardPart(".recording-row-main").textContent = recording.name;
    cardPart(".recording-row-meta").textContent =
      `${when} | ${formatRecordingSize(recording.bytes)} | ${recording.status}`;
    cardPart(".recording-row").addEventListener("click", () => {
      void invoke("recording_open", { recordingId: recording.id }).catch((error) => {
        showToast(errorMessage(error, "Could not open recording"));
      });
    });

    const panel = cardPart(".recording-transcript");
    const transcribeButton = cardPart<HTMLButtonElement>(".recording-transcribe");
    if (transcript) {
      transcribeButton.remove();
      // A single transcript surface serves as both the collapsed preview
      // (CSS line-clamp + fade) and the fully revealed text. Expanding does
      // not render a second copy — the disclosure toggle only unclamps it.
      const transcriptText = cardPart(".recording-transcript-text");
      transcriptText.id = `recording-transcript-${recording.id}`;
      transcriptText.textContent = transcript;

      const copyButton = cardPart(".recording-copy");
      copyButton.setAttribute("aria-label", `Copy transcript for ${recording.name}`);
      copyButton.addEventListener("click", (event) => {
        event.stopPropagation();
        void invoke("copy_to_clipboard", { text: transcript })
          .then(() => showToast("Transcript copied to clipboard"))
          .catch((error) => showToast(errorMessage(error, "Could not copy transcript")));
      });

      const transcriptToggle = cardPart<HTMLButtonElement>(".recording-transcript-toggle");
      const toggleLabel = cardPart(".recording-transcript-toggle-label");
      transcriptToggle.setAttribute("aria-controls", transcriptText.id);
      transcriptToggle.addEventListener("click", () => {
        const expanded = panel.dataset.expanded === "true";
        const next = !expanded;
        panel.dataset.expanded = String(next);
        transcriptToggle.setAttribute("aria-expanded", String(next));
        toggleLabel.textContent = next ? "Show less" : "Show more";
        transcriptToggle.title = next ? "Collapse transcript" : "Show full transcript";
        if (menuOpen) void applyOverlayLayout(true);
      });

      // Only offer the disclosure when the preview is actually truncated.
      // Measured after layout so short transcripts read as plain text.
      requestAnimationFrame(() => {
        const clamped = transcriptText.scrollHeight - transcriptText.clientHeight > 2;
        panel.dataset.clamped = String(clamped);
        transcriptToggle.hidden = !clamped;
        if (clamped && menuOpen) void applyOverlayLayout(true);
      });
    } else {
      panel.remove();
      transcribeButton.title = `Transcribe ${recording.name}`;
      transcribeButton.setAttribute("aria-label", `Transcribe ${recording.name}`);
      transcribeButton.disabled =
        dictation.state !== "idle" || recording.id === dictation.currentRecordingId;
      transcribeButton.addEventListener("click", () => {
        void dictation.retranscribeRecording(recording.id);
      });
    }
    recentRecordings.appendChild(card);
  }
  if (recordings.length === 0) {
    const empty = document.createElement("div");
    empty.className = "recordings-empty";
    empty.textContent = "No saved recordings yet";
    recentRecordings.appendChild(empty);
  }
  await applyOverlayLayout(true);
}

failureOpenRecording.addEventListener("click", () => {
  const recordingId = dictation.lastFailedRecordingId;
  if (!recordingId) return;
  void invoke("recording_open", { recordingId }).catch((error) => {
    showToast(errorMessage(error, "Could not open recording"));
  });
});

failureDismiss.addEventListener("click", () => {
  failurePanel.classList.add("hidden");
  void applyOverlayLayout(true);
});

menuOpenRecordings.addEventListener("click", () => {
  void invoke("recordings_open_folder").catch((error) => {
    showToast(errorMessage(error, "Could not open recordings folder"));
  });
});

menuOpenLogs.addEventListener("click", () => {
  void invoke("logs_open_folder").catch((error) => {
    showToast(errorMessage(error, "Could not open log folder"));
  });
});

menuImportAudio.addEventListener("click", () => void dictation.transcribeImportedFile());

btnMic.addEventListener("click", (e) => {
  e.stopPropagation();
  void dictation.toggleRecording();
});

// ---------------------------------------------------------------------------
// Settings (API key)
// ---------------------------------------------------------------------------
function applyStoredKeyPreview(preview: string | null) {
  storedKeyPreview = preview ?? "";
  showingStoredKeyPreview = storedKeyPreview.length > 0;
  keyInputDirty = false;
  menuKeyInput.type = showingStoredKeyPreview ? "text" : "password";
  menuKeyInput.value = storedKeyPreview;
}

function selectStoredKeyPreview() {
  if (!showingStoredKeyPreview) return;
  requestAnimationFrame(() => menuKeyInput.select());
}

function restoreStoredKeyPreview() {
  if (keyInputDirty) return;
  applyStoredKeyPreview(storedKeyPreview || null);
}

function syncRefinePromptVisibility() {
  const enabled = menuRefineToggle.checked;
  menuRefinePromptInput.style.display = enabled ? "" : "none";
  menuRefinePromptInput.nextElementSibling?.setAttribute(
    "style",
    enabled ? "" : "display:none",
  );
  if (menuOpen) {
    void applyOverlayLayout();
  }
}

function syncModelHint() {
  const model = menuModelSelect.value as TranscriptionModel;
  menuModelHint.textContent = transcriptionModelHint(model);
}

function applySettingsUi(settings: AppSettings, configuredLabel = "Key configured") {
  appSettings = settings;
  const bubbleWasVisible = settingsLoaded && !hasOpenaiApiKey && dictation.state === "idle";
  hasOpenaiApiKey = settings.hasOpenaiApiKey;
  settingsLoaded = true;
  const bubbleVisibilityChanged = bubbleWasVisible !== (!hasOpenaiApiKey && dictation.state === "idle");
  syncApiKeyBubble();
  applyStoredKeyPreview(settings.openaiApiKeyPreview);
  menuKeyStatus.textContent = settings.hasOpenaiApiKey ? configuredLabel : "Not configured";
  menuKeyStatus.className = `menu-item ${settings.hasOpenaiApiKey ? "key-ok" : "key-missing"}`;
  menuModelSelect.value = settings.transcriptionModel ?? DEFAULT_TRANSCRIPTION_MODEL;
  showIdlePill = settings.showIdlePill ?? true;
  menuShowIdlePillToggle.checked = showIdlePill;
  menuHoldToTalkToggle.checked = settings.holdToTalk ?? true;
  syncModelHint();
  if (!promptDirty) {
    menuPromptInput.value = settings.transcriptionPrompt ?? "";
  }
  if (!refineDirty) {
    menuRefineToggle.checked = settings.refineEnabled ?? true;
    menuRefinePromptInput.value = settings.refinePrompt ?? "";
    syncRefinePromptVisibility();
  }
  const requestedLayout = windowLayoutQueue.newestRequested();
  if ((bubbleVisibilityChanged || requestedLayout?.visible !== getWindowLayout().visible) && requestedLayout !== null) {
    void applyOverlayLayout();
  }
}

const { saveSettings, isSaving } = createSettingsSaver<AppSettings>({
  save: (patch) => invoke<AppSettings>("app_save_settings", patch),
  apply: applySettingsUi,
  showToast,
});

async function persistKeyInput() {
  if (!keyInputDirty) return;
  await saveSettings({ openaiApiKey: menuKeyInput.value.trim() }, {
    busy: "key",
    label: (updated) => (updated.hasOpenaiApiKey ? "Key saved" : "Key cleared"),
    done: (updated) => (updated.hasOpenaiApiKey ? "API key saved" : "API key cleared"),
    onError: () => { keyInputDirty = true; },
  });
}

async function revealStoredKeyForEdit() {
  if (!showingStoredKeyPreview || keyRevealInFlight) return;

  keyRevealInFlight = true;
  showingStoredKeyPreview = false;
  menuKeyInput.type = "text";
  requestAnimationFrame(() => menuKeyInput.select());

  try {
    const key = await invoke<string>("app_get_openai_api_key");
    if (keyInputDirty || document.activeElement !== menuKeyInput) {
      return;
    }

    menuKeyInput.value = key;
    requestAnimationFrame(() => menuKeyInput.select());
  } catch (err) {
    if (!keyInputDirty) {
      restoreStoredKeyPreview();
    }
    showToast(errorMessage(err, "Could not read the API key"));
  } finally {
    keyRevealInFlight = false;
  }
}

async function loadSettings() {
  try {
    const settings = await invoke<AppSettings>("app_get_settings");
    applySettingsUi(settings);
  } catch {
    applySettingsUi({ hasOpenaiApiKey: false, openaiApiKeyPreview: null, transcriptionModel: DEFAULT_TRANSCRIPTION_MODEL, transcriptionPrompt: "", refineEnabled: true, refinePrompt: "", showIdlePill: true, holdToTalk: true });
    // Those are placeholders, so a recording asks the app again.
    appSettings = null;
  }
}

async function currentSettings(): Promise<AppSettings> {
  // While the model saves, the page keeps an older answer back so it does not
  // undo a newer choice; ask the app, so the recording uses the model it will.
  if (isSaving("model")) return invoke<AppSettings>("app_get_settings");
  return appSettings ?? invoke<AppSettings>("app_get_settings");
}

menuShowIdlePillToggle.addEventListener("change", () =>
  saveSettings({ showIdlePill: menuShowIdlePillToggle.checked }, {
    busy: "showIdlePill",
    done: (updated) => (updated.showIdlePill ? "Idle pill shown" : "Idle pill hidden"),
    onError: loadSettings,
  }),
);

menuHoldToTalkToggle.addEventListener("change", () =>
  saveSettings({ holdToTalk: menuHoldToTalkToggle.checked }, {
    busy: "holdToTalk",
    done: (updated) => (updated.holdToTalk ? "Hold to talk enabled" : "Press to start and stop enabled"),
    onError: loadSettings,
  }),
);

menuModelSelect.addEventListener("change", () => {
  syncModelHint();
  return saveSettings({ transcriptionModel: menuModelSelect.value }, {
    busy: "model",
    done: () => "Transcription model saved",
    onError: loadSettings,
  });
});

async function ensureAutostartEnabled() {
  try {
    if (!(await isAutostartEnabled())) {
      await enableAutostart();
      logDebug("autostart enabled");
    }
  } catch (err) {
    logDebug(`autostart enable failed=${errorMessage(err, "unknown error")}`);
  }
}

function editStoredKey() {
  selectStoredKeyPreview();
  void revealStoredKeyForEdit();
}

menuKeyInput.addEventListener("focus", editStoredKey);
menuKeyInput.addEventListener("click", editStoredKey);

menuKeyInput.addEventListener("beforeinput", () => {
  if (showingStoredKeyPreview) {
    showingStoredKeyPreview = false;
    menuKeyInput.type = "text";
    keyInputDirty = true;
  }
});

menuKeyInput.addEventListener("input", () => {
  if (!showingStoredKeyPreview) {
    keyInputDirty = true;
  }
});

menuKeyInput.addEventListener("blur", () => {
  if (!keyInputDirty) {
    restoreStoredKeyPreview();
    return;
  }
  void persistKeyInput();
});

menuKeyInput.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    event.preventDefault();
    menuKeyInput.blur();
  }
});

menuKeyLink.addEventListener("click", async (event) => {
  event.preventDefault();
  try {
    await invoke("open_api_keys_page");
  } catch (err) {
    showToast(errorMessage(err, "Could not open the API keys page"));
  }
});

// ---------------------------------------------------------------------------
// Settings (Transcription Prompt)
// ---------------------------------------------------------------------------
async function persistPromptInput() {
  if (!promptDirty) return;
  await saveSettings({ transcriptionPrompt: menuPromptInput.value }, {
    busy: "prompt",
    done: () => "Transcription prompt saved",
    saved: () => { promptDirty = false; },
    onError: () => { promptDirty = true; },
  });
}

menuPromptInput.addEventListener("input", () => {
  promptDirty = true;
});

menuPromptInput.addEventListener("blur", () => void persistPromptInput());

// ---------------------------------------------------------------------------
// Settings (Refine)
// ---------------------------------------------------------------------------
async function persistRefineInput() {
  if (!refineDirty) return;
  await saveSettings({ refineEnabled: menuRefineToggle.checked, refinePrompt: menuRefinePromptInput.value }, {
    busy: "refine",
    saved: () => { refineDirty = false; },
    onError: () => { refineDirty = true; },
  });
}

menuRefineToggle.addEventListener("change", () => {
  refineDirty = true;
  syncRefinePromptVisibility();
  void persistRefineInput();
});

menuRefinePromptInput.addEventListener("input", () => {
  refineDirty = true;
});

menuRefinePromptInput.addEventListener("blur", () => void persistRefineInput());

// ---------------------------------------------------------------------------
// Foreground app name: read each time the menu opens. Until the lookup
// answers, the label says "Detecting…" rather than the last menu's program.
// ---------------------------------------------------------------------------
function showForegroundApp(name: string | null) {
  menuTargetAppName.textContent = name ?? "Unknown";
}

async function refreshForegroundApp() {
  menuTargetAppName.textContent = "Detecting…";
  try {
    showForegroundApp(await invoke<string | null>("get_foreground_app"));
  } catch {
    showForegroundApp(null);
  }
}

// ---------------------------------------------------------------------------
// Tray / global shortcuts: listen for recording events emitted by Rust
// ---------------------------------------------------------------------------
void listen("toggle-recording", () => {
  logDebug("event toggle-recording");
  return void dictation.toggleRecording();
});
void listen("press-to-talk-start", () => {
  logDebug("event press-to-talk-start");
  return void dictation.pressToTalkStart();
});
void listen("press-to-talk-stop", () => {
  logDebug("event press-to-talk-stop");
  return void dictation.pressToTalkStop();
});
void listen<boolean>("overlay-revealed", (event) => {
  manualOverlayReveal = event.payload;
  void collapseOverlay();
});
void listen("overlay-lost-focus", () => {
  manualOverlayReveal = false;
  void collapseOverlay();
});

// ---------------------------------------------------------------------------
// Auto-update
// ---------------------------------------------------------------------------
async function checkForUpdates() {
  let updateAccepted = false;
  try {
    const update = await check();
    if (update) {
      updateAccepted = window.confirm(
        `OpenWhisper ${update.version} is available.\n\n` +
          "Update now? Your settings and API key will be kept.",
      );
      if (!updateAccepted) return;
      showToast("Downloading OpenWhisper update…", 30_000);
      await update.downloadAndInstall();
      await relaunch();
    }
  } catch {
    if (updateAccepted) {
      showToast("Update failed. Your current app and settings are unchanged.", 6000);
    }
    // Ignore background check failures, including development mode.
  }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------
if (import.meta.env.DEV) {
  Object.defineProperty(window, "__openWhisperTest", {
    value: { openMenu, collapseOverlay },
    configurable: true,
  });
}

widget.dataset.state = dictation.state;
syncWidgetFrame();
syncApiKeyBubble();
void loadSettings().then(() => applyOverlayLayout(true));
if (import.meta.env.PROD) {
  void ensureAutostartEnabled();
}
void refreshMicDevices();
// Ready before the first shortcut press; a failure is retried then.
microphoneCapture.warmUp();

if (navigator.mediaDevices?.addEventListener) {
  navigator.mediaDevices.addEventListener("devicechange", () => void refreshMicDevices());
}

// Installed builds update automatically. Development and automated UI runs
// must never replace or relaunch the executable under test.
if (import.meta.env.PROD) {
  setTimeout(() => void checkForUpdates(), 4000);
}
