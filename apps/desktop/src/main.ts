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
  getOverlayLayout,
  isOverlayVisible,
} from "./overlay-layout";
import {
  DEFAULT_TRANSCRIPTION_MODEL,
  isLiveTranscription,
  REFINE_MODEL,
  transcriptionModelHint,
  transcriptionModelOptionsHtml,
  type TranscriptionModel,
} from "./models";
import {
  PCM_FLUSH_REQUEST,
  PCM_FLUSHED,
  PCM_MIME_TYPE,
  PCM_SAMPLE_RATE,
  PCM_WORKLET_NAME,
} from "./pcm-capture";
import pcmCaptureWorkletUrl from "./pcm-capture.worklet?worker&url";
import {
  joinAudioParts,
  sendAudioChunk,
  SessionAudioQueue,
  startCaptureFirst,
} from "./recording-pipeline";
import { finalWriteNotice, type TargetWrite } from "./target-write";
import "./styles.css";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------
type TranscriptEvent = { type: "partial" | "final" | "warning" | "error"; text: string };
// One update the worker pushes for a session; `done` marks its last one.
type WorkerSessionEvent = { session_id: string; event: TranscriptEvent; done?: boolean };
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
// "starting" shows the listening pill the moment the shortcut is pressed,
// while the microphone is still opening.
type WidgetState = "idle" | "starting" | "recording" | "transcribing" | "error";
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
let state: WidgetState = "idle";
let currentSessionId: string | null = null;
// Advanced by every cancel, so a session that was still starting when the
// user cancelled is stopped instead of transcribed.
let cancelCount = 0;
let sessionEventTail: Promise<void> = Promise.resolve();
let finalTranscript = "";
let liveTranscript = "";
let streamTextToTarget = false;
// Live transcription hears audio while it is recorded, so the worker gets
// every chunk. Otherwise audio goes only to the saved backup, and the worker
// reads that file once the recording stops.
let streamAudioToWorker = false;
let targetWriteQueue: Promise<void> = Promise.resolve();
let lastTranscript = localStorage.getItem("ow_last_transcript") ?? "";
// The settings the menu shows. Every change passes through applySettingsUi,
// so a recording reads them here instead of asking the app each time.
let appSettings: AppSettings | null = null;
// Built once at the upload rate with the worklet loaded, then suspended
// between recordings, so the microphone opens without rebuilding it.
let pcmContextReady: Promise<AudioContext> | null = null;
// The context of the capture now running; null when none is.
let pcmAudioContext: AudioContext | null = null;
let pcmSource: MediaStreamAudioSourceNode | null = null;
let pcmProcessor: AudioWorkletNode | null = null;
let pcmSilentGain: GainNode | null = null;
let isRecording = false;
let currentRecordingId: string | null = null;
let lastFailedRecordingId: string | null = null;
let backupTail: Promise<void> = Promise.resolve();
let backupError: unknown = null;
let pendingAudioParts: Uint8Array[] = [];
let pendingAudioBytes = 0;
// Audio captured before the backup file opened, kept by the number of the
// cancel that stopped its recording, so the backup still saves it once it opens.
const audioCancelledBeforeBackup = new Map<number, Uint8Array[]>();
let sessionFailure = "";
let shouldPasteFinalTranscript = true;
let pressToTalkHeld = false;
let pressToTalkStarting = false;
let toggleRecordingStarting = false;
let toggleStopRequestedDuringStart = false;
const audioQueue = new SessionAudioQueue((sessionId, buffer) =>
  sendAudioChunk(invoke, "worker_append_audio_chunk", sessionId, buffer),
);

let micStream: MediaStream | null = null;
let soundVisualizer: { start: () => void; stop: () => void } | null = null;
let volumeMonitorFrameId: number | null = null;
let volumeMonitorDisconnect: (() => void) | null = null;
const SILENT_THRESHOLD = 14;
let micDevices: MicDevice[] = [];
let selectedMicId = "default";
let menuOpen = false;

const HOVER_EXPAND_DELAY_MS = 40;
const HOVER_COLLAPSE_GRACE_MS = 90;
const NOTICE_DURATION_MS = 5000;
// Left inset + status dot + gaps + dropdown button around the notice text.
const NOTICE_CHROME_WIDTH = 52;
const NOTICE_MAX_WIDTH = 180;
const AUDIO_BATCH_BYTES = 16 * 1024;

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

      <span class="meter meter-wave-bars" aria-hidden="true">
        <span class="meter-baseline" aria-hidden="true"></span>
        <svg class="meter-idle-wave" viewBox="0 0 76 28" aria-hidden="true">
          <path class="meter-idle-wave-path" d="M0,14 C6,11 12,17 18,14 C24,11 30,17 36,14 C42,11 48,17 54,14 C60,11 66,17 72,14 C74,12 76,14" fill="none" stroke="currentColor" stroke-width="0.9" stroke-linecap="round" />
        </svg>
        <canvas id="meter-canvas" class="meter-canvas" width="48" height="28"></canvas>
        <span class="meter-idle-bars">
          <span class="wave-bar" data-bar="0"></span>
          <span class="wave-bar" data-bar="1"></span>
          <span class="wave-bar" data-bar="2"></span>
          <span class="wave-bar" data-bar="3"></span>
          <span class="wave-bar" data-bar="4"></span>
          <span class="wave-bar" data-bar="5"></span>
          <span class="wave-bar" data-bar="6"></span>
        </span>
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
      </div>
      <div id="recent-recordings" class="recent-recordings"></div>
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
const waveBars = document.querySelectorAll<HTMLElement>(".wave-bar");
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
const recentRecordings = document.getElementById("recent-recordings")!;
const widgetNoticeText = document.getElementById("widget-notice-text")!;

let hasOpenaiApiKey = false;
let settingsLoaded = false;
let storedKeyPreview = "";
let showingStoredKeyPreview = false;
let keyInputDirty = false;
let keySaveInFlight = false;
let keyRevealInFlight = false;
let promptDirty = false;
let promptSaveInFlight = false;
let modelSaveInFlight = false;
let refineDirty = false;
let refineSaveInFlight = false;
let showIdlePill = true;
let showIdlePillSaveInFlight = false;
let holdToTalkSaveInFlight = false;
let manualOverlayReveal = false;
let widgetHovered = false;
let expandTimeoutId: number | null = null;
let collapseTimeoutId: number | null = null;
let noticeWidth = 0;
let noticeTimeoutId: number | null = null;
let lastAppliedLayout: { width: number; height: number; anchorOffsetY: number; visible: boolean } | null = null;
let overlayLayoutQueue: Promise<void> = Promise.resolve();

function logDebug(message: string) {
  const timestamp = new Date().toISOString();
  void invoke("debug_log", { message: `${timestamp} ${message}` }).catch(() => {});
}

function isWidgetExpanded() {
  return widgetHovered || menuOpen || state !== "idle" || noticeWidth > 0;
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
  widget.dataset.hasLastTranscript = String(lastTranscript.length > 0);
}

function syncApiKeyBubble() {
  const show = settingsLoaded && !hasOpenaiApiKey && state === "idle";
  if (show) {
    apiKeyBubble.classList.remove("hidden");
  } else {
    apiKeyBubble.classList.add("hidden");
  }
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
    expanded: isWidgetExpanded(),
    noticeWidth,
    settingsLoaded,
    hasOpenaiApiKey: hasOpenaiApiKey || state !== "idle",
    menuVisible,
    menuWidth: menuRect?.width,
    menuHeight: menuRect?.height,
  });
  return {
    ...layout,
    visible: isOverlayVisible({
      showIdlePill,
      idle: state === "idle",
      menuOpen,
      manualReveal: manualOverlayReveal,
      noticeShowing: noticeWidth > 0,
    }),
  };
}

async function applyOverlayLayout(force = false) {
  syncWidgetFrame();
  const layout = getWindowLayout();
  if (
    !force &&
    lastAppliedLayout?.width === layout.width &&
    lastAppliedLayout?.height === layout.height &&
    lastAppliedLayout?.anchorOffsetY === layout.anchorOffsetY &&
    lastAppliedLayout?.visible === layout.visible
  ) {
    return;
  }

  lastAppliedLayout = layout;
  const apply = overlayLayoutQueue
    .catch(() => {})
    .then(() => invoke<void>("overlay_apply_layout", {
      width: layout.width,
      height: layout.height,
      anchorOffsetY: layout.anchorOffsetY,
      visible: layout.visible,
    }));
  overlayLayoutQueue = apply;
  await apply;
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

// Opens the picked microphone. One that is gone (headphones switched off or
// unplugged) falls back to the Windows default, which becomes the choice.
async function openMicrophone(constraints: MediaStreamConstraints): Promise<MediaStream> {
  try {
    return await navigator.mediaDevices.getUserMedia(constraints);
  } catch (error) {
    const name = error instanceof DOMException ? error.name : "";
    const deviceGone = name === "OverconstrainedError" || name === "NotFoundError";
    if (selectedMicId === "default" || !deviceGone) throw error;
    logDebug(`picked microphone unavailable (${name}); using the system default`);
    selectedMicId = "default";
    renderMicList();
    return navigator.mediaDevices.getUserMedia({ audio: true });
  }
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
      closeMenu();
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
  void refreshRecordings();
  await applyOverlayLayout();
}

function closeMenu() {
  void collapseOverlay();
}

async function collapseOverlay() {
  menuOpen = false;
  if (expandTimeoutId !== null) {
    clearTimeout(expandTimeoutId);
    expandTimeoutId = null;
  }
  if (collapseTimeoutId !== null) {
    clearTimeout(collapseTimeoutId);
    collapseTimeoutId = null;
  }
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

function handleWidgetClick(e: MouseEvent) {
  if (menuOpen) {
    closeMenu();
    e.stopPropagation();
    e.preventDefault();
    return;
  }

  if (state === "recording" || state === "transcribing") {
    e.stopPropagation();
    e.preventDefault();
    if (state === "recording") {
      void stopRecording();
    } else {
      void cancelTranscription();
    }
    return;
  }
  if (btnMic.contains(e.target as Node) || btnDropdown.contains(e.target as Node)) return;
  void toggleRecording();
}

function handleWidgetKeydown(e: KeyboardEvent) {
  if (state === "recording" || state === "transcribing") {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      if (state === "recording") void stopRecording();
      else {
        void cancelTranscription();
      }
    }
    return;
  }
  if ((e.key === "Enter" || e.key === " ") && !btnMic.contains(e.target as Node) && !btnDropdown.contains(e.target as Node)) {
    e.preventDefault();
    void toggleRecording();
  }
}

widget.addEventListener("click", handleWidgetClick, true);

widget.addEventListener("keydown", handleWidgetKeydown);

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
  if (expandTimeoutId !== null) {
    clearTimeout(expandTimeoutId);
    expandTimeoutId = null;
  }
  if (collapseTimeoutId !== null) {
    clearTimeout(collapseTimeoutId);
    collapseTimeoutId = null;
  }
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
    closeMenu();
  }
});

window.addEventListener("blur", () => {
  if (menuOpen) closeMenu();
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
  logDebug(`setState ${state} -> ${next}`);
  state = next;
  // Starting looks like a recording that has not heard sound yet, so the pill
  // does not change when the microphone comes on.
  widget.dataset.state = next === "starting" ? "recording" : next;
  if (next === "starting") widget.dataset.silent = "true";
  else if (next === "idle") delete widget.dataset.silent;
  syncApiKeyBubble();
  void applyOverlayLayout();
}

// ---------------------------------------------------------------------------
// Meter — scrolling waveform (recording) + idle bars (transcribing)
// ---------------------------------------------------------------------------
const VOLUME_BAR_COUNT = 7;

function setWaveBarLevels(levels: number[]) {
  waveBars.forEach((bar, i) => {
    const level = levels[i] ?? 0.15;
    const clamped = Math.max(0.15, Math.min(level, 1));
    bar.style.setProperty("--wave-level", clamped.toFixed(3));
  });
}

// ---------------------------------------------------------------------------
// Volume monitor — show wobbly line when mic is silent during recording
// ---------------------------------------------------------------------------
// The scrolling waveform while recording. It listens on the running
// capture's source, in the same audio context: a context of its own opened
// the speakers, which woke Bluetooth headphones and froze the page for about
// a second on every recording.
const METER_SLICES = 20;
const METER_BAR_WIDTH = 3;
const METER_COLOR = "#9ef0c9";

function startMeterVisualizer(): { start: () => void; stop: () => void } | null {
  const source = pcmSource;
  if (!source) return null;
  const analyser = source.context.createAnalyser();
  source.connect(analyser);
  const data = new Uint8Array(analyser.frequencyBinCount);
  const history: number[] = [];
  let frameId: number | null = null;

  function draw() {
    const canvas = meterCanvas;
    const context = canvas.getContext("2d");
    if (!context) return;
    const { width, height } = canvas;
    context.fillStyle = METER_COLOR;
    context.clearRect(0, 0, width, height);
    context.beginPath();
    const sliceWidth = width / METER_SLICES;
    // New bars enter on the right and scroll left.
    const firstSlice = METER_SLICES - history.length;
    history.forEach((value, index) => {
      let fraction = value / 255;
      if (fraction < 0.05) fraction += 0.05;
      const top = (height / 2) * fraction;
      context.rect((firstSlice + index) * sliceWidth, top, METER_BAR_WIDTH, height - 2 * top);
    });
    context.fill();
  }

  function tick() {
    analyser.getByteTimeDomainData(data);
    let min = 255;
    let max = 0;
    for (const value of data) {
      if (value < min) min = value;
      if (value > max) max = value;
    }
    history.push(255 - (max - min));
    if (history.length > METER_SLICES) history.shift();
    draw();
    frameId = requestAnimationFrame(tick);
  }

  return {
    start() {
      if (frameId !== null) cancelAnimationFrame(frameId);
      frameId = requestAnimationFrame(tick);
    },
    stop() {
      if (frameId !== null) cancelAnimationFrame(frameId);
      frameId = null;
      try {
        source.disconnect(analyser);
      } catch {
        // Stopping the capture already disconnected the source.
      }
    },
  };
}

// Listens on the running capture's source, in the same audio context.
function startVolumeMonitor() {
  const source = pcmSource;
  if (!source) return;
  const analyser = source.context.createAnalyser();
  analyser.fftSize = 256;
  source.connect(analyser);
  const data = new Uint8Array(analyser.fftSize);

  function tick() {
    if (state !== "recording") return;
    analyser.getByteTimeDomainData(data);
    let min = 255, max = 0;
    for (let i = 0; i < data.length; i++) {
      if (data[i] < min) min = data[i];
      if (data[i] > max) max = data[i];
    }
    const range = max - min;
    widget.dataset.silent = range < SILENT_THRESHOLD ? "true" : "false";
    volumeMonitorFrameId = requestAnimationFrame(tick);
  }
  volumeMonitorFrameId = requestAnimationFrame(tick);

  volumeMonitorDisconnect = () => {
    if (volumeMonitorFrameId !== null) {
      cancelAnimationFrame(volumeMonitorFrameId);
      volumeMonitorFrameId = null;
    }
    try {
      source.disconnect(analyser);
    } catch {
      // Stopping the capture already disconnected the source.
    }
    volumeMonitorDisconnect = null;
    delete widget.dataset.silent;
  };
}

function stopVolumeMonitor() {
  volumeMonitorDisconnect?.();
}

// ---------------------------------------------------------------------------
// Recording
// ---------------------------------------------------------------------------
function queueAudioChunk(buffer: ArrayBuffer) {
  pendingAudioParts.push(new Uint8Array(buffer));
  pendingAudioBytes += buffer.byteLength;
  if (pendingAudioBytes >= AUDIO_BATCH_BYTES) flushAudioBatch();
}

function flushAudioBatch() {
  // Audio captured while the backup file is still opening waits here, and
  // is saved first once it opens.
  const recordingId = currentRecordingId;
  if (pendingAudioBytes === 0 || !recordingId) return;
  const combined = joinAudioParts(pendingAudioParts);
  pendingAudioParts = [];
  pendingAudioBytes = 0;

  backupTail = backupTail
    .then(() => sendAudioChunk(invoke, "recording_append_chunk", recordingId, combined))
    .catch((error) => {
      backupError ??= error;
    });
  if (streamAudioToWorker) audioQueue.append(combined.buffer);
}

function errorMessage(error: unknown, fallback = "Transcription failed"): string {
  if (error instanceof Error && error.message.trim()) return error.message;
  const message = String(error ?? "").trim();
  return message || fallback;
}

async function finishCurrentRecording(status: string, transcript: string, error: string) {
  const recordingId = currentRecordingId;
  if (!recordingId) return;
  currentRecordingId = null;
  try {
    await invoke("recording_finish", {
      recordingId,
      status,
      transcript,
      error,
    });
  } catch (finishError) {
    logDebug(`recording metadata update failed=${errorMessage(finishError)}`);
  }
  void refreshRecordings();
}

async function handleTranscriptionFailure(error: unknown) {
  if (pcmAudioContext) await stopPcmCapture();
  flushAudioBatch();
  await backupTail;
  const message = errorMessage(error);
  const recordingId = currentRecordingId;
  if (recordingId) {
    lastFailedRecordingId = recordingId;
    await finishCurrentRecording("failed", "", message);
  }
  const sentence = /[.!?]$/.test(message) ? message : `${message}.`;
  failureMessage.textContent = recordingId
    ? `${sentence} The raw audio was saved locally and can be opened below.`
    : message;
  failureOpenRecording.disabled = !lastFailedRecordingId;
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

async function refreshRecordings() {
  try {
    const recordings = await invoke<RecordingMetadata[]>("recording_list");
    recentRecordings.replaceChildren();
    for (const recording of recordings.slice(0, 4)) {
      const transcript = recording.transcript.trim();
      const card = document.createElement("div");
      card.className = "recording-card";
      const audioButton = document.createElement("button");
      audioButton.type = "button";
      audioButton.className = "recording-row";
      const when = new Date(recording.createdAtMs).toLocaleString([], {
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      });
      const main = document.createElement("span");
      main.className = "recording-row-main";
      main.textContent = recording.name;
      const meta = document.createElement("span");
      meta.className = "recording-row-meta";
      meta.textContent = `${when} | ${formatRecordingSize(recording.bytes)} | ${recording.status}`;
      audioButton.append(main, meta);
      audioButton.title = "Open raw audio";
      audioButton.addEventListener("click", () => {
        void invoke("recording_open", { recordingId: recording.id }).catch((error) => {
          showToast(errorMessage(error, "Could not open recording"));
        });
      });

      card.appendChild(audioButton);
      if (transcript) {
        // A single transcript surface serves as both the collapsed preview
        // (CSS line-clamp + fade) and the fully revealed text. Expanding does
        // not render a second copy — the disclosure toggle only unclamps it.
        const panel = document.createElement("div");
        panel.className = "recording-transcript";
        panel.dataset.expanded = "false";
        panel.dataset.clamped = "false";

        const surface = document.createElement("div");
        surface.className = "recording-transcript-surface";

        const transcriptText = document.createElement("div");
        transcriptText.id = `recording-transcript-${recording.id}`;
        transcriptText.className = "recording-transcript-text";
        transcriptText.textContent = transcript;

        const copyButton = document.createElement("button");
        copyButton.type = "button";
        copyButton.className = "recording-copy";
        copyButton.title = "Copy transcript";
        copyButton.setAttribute("aria-label", `Copy transcript for ${recording.name}`);
        copyButton.innerHTML = `
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <rect x="9" y="9" width="11" height="11" rx="2.4" />
            <path d="M6 15H5.4A1.6 1.6 0 0 1 4 13.4V5.4A1.6 1.6 0 0 1 5.4 4h8A1.6 1.6 0 0 1 15 5.4V6" />
          </svg>`;
        copyButton.addEventListener("click", (event) => {
          event.stopPropagation();
          void invoke("copy_to_clipboard", { text: transcript })
            .then(() => showToast("Transcript copied to clipboard"))
            .catch((error) => showToast(errorMessage(error, "Could not copy transcript")));
        });
        surface.append(transcriptText, copyButton);

        const transcriptToggle = document.createElement("button");
        transcriptToggle.type = "button";
        transcriptToggle.className = "recording-transcript-toggle";
        transcriptToggle.setAttribute("aria-expanded", "false");
        transcriptToggle.setAttribute("aria-controls", transcriptText.id);
        transcriptToggle.title = "Show full transcript";
        transcriptToggle.hidden = true;
        const toggleLabel = document.createElement("span");
        toggleLabel.className = "recording-transcript-toggle-label";
        toggleLabel.textContent = "Show more";
        transcriptToggle.append(toggleLabel);
        transcriptToggle.insertAdjacentHTML(
          "beforeend",
          `<svg class="recording-transcript-chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M7.5 10.5 12 15 16.5 10.5" /></svg>`,
        );
        transcriptToggle.addEventListener("click", () => {
          const expanded = panel.dataset.expanded === "true";
          const next = !expanded;
          panel.dataset.expanded = String(next);
          transcriptToggle.setAttribute("aria-expanded", String(next));
          toggleLabel.textContent = next ? "Show less" : "Show more";
          transcriptToggle.title = next ? "Collapse transcript" : "Show full transcript";
          if (menuOpen) void applyOverlayLayout(true);
        });

        panel.append(surface, transcriptToggle);
        card.appendChild(panel);

        // Only offer the disclosure when the preview is actually truncated.
        // Measured after layout so short transcripts read as plain text.
        requestAnimationFrame(() => {
          const clamped = transcriptText.scrollHeight - transcriptText.clientHeight > 2;
          panel.dataset.clamped = String(clamped);
          transcriptToggle.hidden = !clamped;
          if (clamped && menuOpen) void applyOverlayLayout(true);
        });
      } else {
        const transcribeButton = document.createElement("button");
        transcribeButton.type = "button";
        transcribeButton.className = "recording-transcribe";
        transcribeButton.textContent = "Transcribe";
        transcribeButton.title = `Transcribe ${recording.name}`;
        transcribeButton.setAttribute("aria-label", `Transcribe ${recording.name}`);
        transcribeButton.disabled = state !== "idle" || recording.id === currentRecordingId;
        transcribeButton.addEventListener("click", () => {
          void retranscribeRecording(recording);
        });
        card.appendChild(transcribeButton);
      }
      recentRecordings.appendChild(card);
    }
    if (recordings.length === 0) {
      const empty = document.createElement("div");
      empty.className = "recordings-empty";
      empty.textContent = "No saved recordings yet";
      recentRecordings.appendChild(empty);
    }
    if (menuOpen) await applyOverlayLayout(true);
  } catch (error) {
    logDebug(`recording list failed=${errorMessage(error)}`);
  }
}

failureOpenRecording.addEventListener("click", () => {
  if (!lastFailedRecordingId) return;
  void invoke("recording_open", { recordingId: lastFailedRecordingId }).catch((error) => {
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

menuImportAudio.addEventListener("click", () => void transcribeImportedFile());

// The context runs at the upload rate, so the browser's filtered resampler
// converts the microphone audio. The worklet only packs it as 16-bit PCM, on
// the audio thread, so a busy page cannot drop samples.
//
// Capture never plays sound, so the context opens no speaker ("none" sink).
// Opening the default speaker woke Bluetooth headphones on every recording,
// which held up the microphone by a second or more. Runtimes without this
// option ignore it and use the default speaker as before.
const CAPTURE_CONTEXT_OPTIONS = {
  sampleRate: PCM_SAMPLE_RATE,
  sinkId: { type: "none" },
} as AudioContextOptions & { sinkId: { type: "none" } };

function pcmCaptureContext(): Promise<AudioContext> {
  if (!pcmContextReady) {
    const ready = (async () => {
      const context = new AudioContext(CAPTURE_CONTEXT_OPTIONS);
      try {
        if (context.sampleRate !== PCM_SAMPLE_RATE) {
          throw new Error(
            `Audio capture runs at ${context.sampleRate} Hz, not ${PCM_SAMPLE_RATE} Hz`,
          );
        }
        await context.audioWorklet.addModule(pcmCaptureWorkletUrl);
        await context.suspend();
        return context;
      } catch (error) {
        void context.close();
        throw error;
      }
    })();
    pcmContextReady = ready;
    // A failed build is tried again at the next recording.
    ready.catch(() => {
      if (pcmContextReady === ready) pcmContextReady = null;
    });
  }
  return pcmContextReady;
}

async function startPcmCapture(stream: MediaStream) {
  let context = await pcmCaptureContext();
  if (context.state === "closed") {
    pcmContextReady = null;
    context = await pcmCaptureContext();
  }
  await context.resume();
  const source = context.createMediaStreamSource(stream);
  let processor: AudioWorkletNode;
  try {
    processor = new AudioWorkletNode(context, PCM_WORKLET_NAME, {
      numberOfInputs: 1,
      numberOfOutputs: 1,
      outputChannelCount: [1],
      channelCount: 1,
      channelCountMode: "explicit",
      channelInterpretation: "speakers",
    });
  } catch (error) {
    void context.suspend();
    throw error;
  }
  processor.port.onmessage = (event: MessageEvent<ArrayBuffer | string>) => {
    if (event.data instanceof ArrayBuffer) queueAudioChunk(event.data);
  };
  // A silent path to the speakers keeps the browser pulling audio through
  // the worklet.
  const silentGain = context.createGain();
  silentGain.gain.value = 0;
  source.connect(processor);
  processor.connect(silentGain);
  silentGain.connect(context.destination);
  pcmAudioContext = context;
  pcmSource = source;
  pcmProcessor = processor;
  pcmSilentGain = silentGain;
}

// Asks the worklet for the samples that did not yet fill a chunk, so the end
// of the recording is kept. Gives up after a short wait.
function flushPcmWorklet(processor: AudioWorkletNode): Promise<void> {
  return new Promise((resolve) => {
    const timeout = window.setTimeout(resolve, 250);
    processor.port.onmessage = (event: MessageEvent<ArrayBuffer | string>) => {
      if (event.data instanceof ArrayBuffer) {
        queueAudioChunk(event.data);
      } else if (event.data === PCM_FLUSHED) {
        window.clearTimeout(timeout);
        resolve();
      }
    };
    processor.port.postMessage(PCM_FLUSH_REQUEST);
  });
}

// keepTail is false when the recording is being thrown away.
async function stopPcmCapture(keepTail = true) {
  const processor = pcmProcessor;
  if (processor && keepTail && pcmAudioContext?.state === "running") {
    await flushPcmWorklet(processor);
    // Another stop finished this capture while the tail was on its way.
    if (pcmProcessor !== processor) return;
  }
  if (processor) processor.port.onmessage = null;
  pcmSource?.disconnect();
  pcmProcessor?.disconnect();
  pcmSilentGain?.disconnect();
  pcmSource = null;
  pcmProcessor = null;
  pcmSilentGain = null;
  const context = pcmAudioContext;
  pcmAudioContext = null;
  micStream?.getTracks().forEach((track) => track.stop());
  isRecording = false;
  // Kept for the next recording; suspended, it does no work meanwhile. One
  // the system closed is rebuilt when the next recording starts.
  if (context) await context.suspend().catch(() => undefined);
}

async function startRecording() {
  const startupStartedAt = performance.now();
  logDebug(
    `startRecording enter state=${state} held=${pressToTalkHeld} starting=${pressToTalkStarting}`,
  );
  if (state !== "idle") return;
  const cancelsBefore = cancelCount;
  // Show the pill before anything is awaited, so it appears as the shortcut
  // is pressed. A missing key, when already known, shows only its toast.
  // The notice is cleared after, so a pill already showing one stays up.
  if (appSettings?.hasOpenaiApiKey !== false) setState("starting");
  clearWidgetNotice();

  try {
    const settings = await currentSettings();
    if (!settings.hasOpenaiApiKey) {
      showToast("Set your OpenAI API key first (click arrow)");
      setState("idle");
      return;
    }
    streamAudioToWorker = isLiveTranscription(settings.transcriptionModel);

    const constraints: MediaStreamConstraints = {
      audio: selectedMicId === "default" ? true : { deviceId: { exact: selectedMicId } },
    };
    currentRecordingId = null;
    backupTail = Promise.resolve();
    backupError = null;
    pendingAudioParts = [];
    pendingAudioBytes = 0;
    sessionFailure = "";
    shouldPasteFinalTranscript = true;

    const result = await startCaptureFirst({
      lockPasteTarget: () => invoke("capture_paste_target"),
      // Capture is deliberately attached before the backup file, the worker
      // or any network startup. Its opening chunks wait until those are ready.
      startCapture: async () => {
        micStream = await openMicrophone(constraints);
        await startPcmCapture(micStream);
        isRecording = true;
        return micStream;
      },
      onCaptureStarted: async (stream) => {
        setState("recording");
        widget.dataset.silent = "true";
        soundVisualizer = startMeterVisualizer();
        soundVisualizer?.start();
        startVolumeMonitor();
        logDebug(`capture ready startup_ms=${Math.round(performance.now() - startupStartedAt)}`);

        // A hold shortcut may be released, or a toggle shortcut may be pressed
        // again, while microphone permission/capture is still starting.
        if (
          (pressToTalkStarting && !pressToTalkHeld) ||
          toggleStopRequestedDuringStart
        ) {
          await stopRecording();
        }
      },
      openBackup: () => invoke<{ id: string }>("recording_begin", {
        source: "microphone",
        name: `Recording ${new Date().toLocaleString()}`,
      }),
      onBackupOpened: async (backup) => {
        if (cancelCount !== cancelsBefore) {
          // Cancelled while the file was opening; the cancel already tidied
          // up, and a new recording may have started since.
          const audio = audioCancelledBeforeBackup.get(cancelsBefore + 1);
          audioCancelledBeforeBackup.delete(cancelsBefore + 1);
          if (audio?.length) {
            await sendAudioChunk(
              invoke,
              "recording_append_chunk",
              backup.id,
              joinAudioParts(audio),
            ).catch((error) => {
              logDebug(`recording append failed=${errorMessage(error)}`);
            });
          }
          await invoke("recording_finish", {
            recordingId: backup.id,
            status: "cancelled",
            transcript: "",
            error: "Cancelled by user",
          }).catch((error) => {
            logDebug(`recording metadata update failed=${errorMessage(error)}`);
          });
          void refreshRecordings();
          return false;
        }
        currentRecordingId = backup.id;
        flushAudioBatch();
        return true;
      },
      startSession: () => invoke<{ session_id: string }>("worker_start_session", {
        profileId: "default",
      }),
    });
    if (!result) return;
    if (cancelCount !== cancelsBefore) {
      // Cancelled while the session was starting; the cancel already tidied up.
      await invoke("worker_stop_session", { sessionId: result.session_id }).catch((error) => {
        logDebug(`stop session failed=${errorMessage(error)}`);
      });
      return;
    }
    currentSessionId = result.session_id;
    audioQueue.attachSession(currentSessionId);
    finalTranscript = "";
    liveTranscript = "";
    streamTextToTarget = isLiveTranscription(settings.transcriptionModel);
    targetWriteQueue = Promise.resolve();
    logDebug(`session ready startup_ms=${Math.round(performance.now() - startupStartedAt)}`);

    if (state === "transcribing") {
      await finalizeRecording();
      return;
    }
    logDebug(`startRecording ready session=${currentSessionId ?? "none"} held=${pressToTalkHeld}`);
  } catch (err) {
    audioCancelledBeforeBackup.delete(cancelsBefore + 1);
    logDebug(`startRecording error=${err instanceof Error ? err.message : String(err)}`);
    if (pcmAudioContext) await stopPcmCapture();
    flushAudioBatch();
    await backupTail;
    await handleTranscriptionFailure(err);
    cleanup();
    setState("idle");
  }
}

async function stopRecording() {
  logDebug(`stopRecording enter state=${state} session=${currentSessionId ?? "none"}`);
  if (state !== "recording") return;
  setState("transcribing");
  if (soundVisualizer) {
    soundVisualizer.stop();
    soundVisualizer = null;
  }
  stopVolumeMonitor();
  setWaveBarLevels(Array(VOLUME_BAR_COUNT).fill(0.5));

  if (pcmAudioContext) {
    await stopPcmCapture();
    flushAudioBatch();
  }

  if (currentSessionId) await finalizeRecording();
}

async function finalizeRecording() {
  const sessionId = currentSessionId;
  if (!sessionId) return;
  try {
    await audioQueue.drain();
    await backupTail;
    if (backupError) throw backupError;
    if (currentSessionId !== sessionId) return;
    if (streamAudioToWorker) {
      await invoke("worker_finalize_session_audio", {
        sessionId,
        mimeType: PCM_MIME_TYPE,
      });
    } else {
      // The worker reads the saved backup from disk, so the audio crosses
      // to it once, at the end, instead of a second time while recording.
      const recordingId = currentRecordingId;
      if (!recordingId) throw new Error("The recording was not saved");
      await invoke("recording_transcribe", { recordingId, sessionId });
    }
    logDebug(`stopRecording finalized session=${sessionId}`);
  } catch (err) {
    logDebug(`stopRecording error=${err instanceof Error ? err.message : String(err)}`);
    // Cancel answers at once, so a cancelled session's last request can
    // still fail here; the cancel already tidied up.
    if (currentSessionId !== sessionId) return;
    await handleTranscriptionFailure(err);
    cleanup();
    setState("idle");
  }
}

async function cancelTranscription() {
  cancelCount += 1;
  if (!currentRecordingId && pendingAudioParts.length > 0) {
    // The backup is still opening; it saves this audio once it opens.
    audioCancelledBeforeBackup.set(cancelCount, pendingAudioParts);
  }
  const sessionId = currentSessionId;
  if (sessionId) {
    await invoke("worker_stop_session", { sessionId }).catch((error) => {
      logDebug(`stop session failed=${errorMessage(error)}`);
    });
  }
  // Capture has stopped; let its last pieces reach the backup before closing it.
  await backupTail;
  await finishCurrentRecording("cancelled", "", "Cancelled by user");
  cleanup();
  setState("idle");
  showToast("Transcription cancelled. Raw audio is still saved.", 4000);
}

async function transcribeImportedFile() {
  if (state !== "idle") {
    showToast("Finish the current recording first");
    return;
  }
  try {
    const settings = await currentSettings();
    if (!settings.hasOpenaiApiKey) {
      throw new Error("Set your OpenAI API key before importing audio");
    }
    if (isLiveTranscription(settings.transcriptionModel)) {
      throw new Error("Choose Standard transcription for imported audio files");
    }
    // The native picker hands the path to the app, which copies the file
    // from disk; the audio never passes through this window.
    const stored = await invoke<{ id: string } | null>("recording_import");
    if (!stored) return;
    if (state !== "idle") {
      showToast("Finish the current recording first");
      return;
    }
    await transcribeStoredAudio(stored.id, "file-import");
  } catch (error) {
    await handleTranscriptionFailure(error);
    cleanup();
    setState("idle");
  }
}

async function retranscribeRecording(recording: RecordingMetadata) {
  if (state !== "idle") {
    showToast("Finish the current recording first");
    return;
  }
  try {
    const settings = await currentSettings();
    if (!settings.hasOpenaiApiKey) {
      throw new Error("Set your OpenAI API key before transcribing saved audio");
    }
    if (isLiveTranscription(settings.transcriptionModel)) {
      throw new Error("Choose Standard transcription for saved audio files");
    }
    await transcribeStoredAudio(recording.id, "recording-retry");
  } catch (error) {
    await handleTranscriptionFailure(error);
    cleanup();
    setState("idle");
  }
}

async function transcribeStoredAudio(recordingId: string, profileId: string) {
  clearWidgetNotice();
  currentRecordingId = recordingId;
  lastFailedRecordingId = null;
  sessionFailure = "";
  shouldPasteFinalTranscript = false;
  finalTranscript = "";
  liveTranscript = "";
  targetWriteQueue = Promise.resolve();
  await invoke("recording_finish", {
    recordingId,
    status: "transcribing",
    transcript: "",
    error: "",
  });
  setState("transcribing");
  closeMenu();

  const cancelsBefore = cancelCount;
  const session = await invoke<{ session_id: string }>("worker_start_session", {
    profileId,
  });
  const sessionId = session.session_id;
  if (cancelCount !== cancelsBefore) {
    // Cancelled while the session was starting; the cancel already tidied up.
    await invoke("worker_stop_session", { sessionId }).catch((error) => {
      logDebug(`stop session failed=${errorMessage(error)}`);
    });
    return;
  }
  currentSessionId = sessionId;
  const cancelled = () => currentSessionId !== sessionId;
  try {
    // The app finds the saved file and the worker reads it from disk, so
    // the audio is never loaded into this window.
    await invoke("recording_transcribe", { recordingId, sessionId });
  } catch (error) {
    // A cancel during the transcription already tidied up; its failure is expected.
    if (cancelled()) return;
    throw error;
  }
}

function cleanup() {
  logDebug(`cleanup state=${state} session=${currentSessionId ?? "none"}`);
  if (soundVisualizer) {
    soundVisualizer.stop();
    soundVisualizer = null;
  }
  stopVolumeMonitor();
  if (pcmAudioContext) void stopPcmCapture(false);
  micStream?.getTracks().forEach((t) => t.stop());
  micStream = null;
  currentSessionId = null;
  audioQueue.reset();
  pendingAudioParts = [];
  pendingAudioBytes = 0;
  liveTranscript = "";
  streamTextToTarget = false;
  streamAudioToWorker = false;
}

// ---------------------------------------------------------------------------
// Session events: the worker pushes progress, live text and the result as
// they happen. They are handled one at a time, in the order they arrive.
// ---------------------------------------------------------------------------
function queueSessionWork(work: () => Promise<void>) {
  sessionEventTail = sessionEventTail.then(work).catch((error) => {
    logDebug(`session event failed=${errorMessage(error)}`);
  });
}

void listen<WorkerSessionEvent>("worker-session-event", ({ payload }) => {
  queueSessionWork(() => handleSessionEvent(payload));
});
void listen<string>("worker-exited", ({ payload }) => {
  queueSessionWork(() => handleWorkerExit(payload));
});

async function handleSessionEvent(message: WorkerSessionEvent) {
  if (!currentSessionId || message.session_id !== currentSessionId) return;
  try {
    await applyTranscriptEvent(message.event);
    if (message.done) await completeSession(message.session_id);
  } catch (error) {
    const text = errorMessage(error, "Could not finish the transcription");
    logDebug(`session event failed=${text}`);
    if (message.done && (state === "recording" || state === "transcribing")) {
      await handleTranscriptionFailure(text);
      cleanup();
      setState("idle");
    }
  }
}

async function applyTranscriptEvent(ev: TranscriptEvent) {
  if (ev.type === "final") {
    finalTranscript = `${finalTranscript} ${ev.text}`.trim();
    liveTranscript = ev.text;
    widget.dataset.liveTranscript = liveTranscript;
    widget.setAttribute("aria-label", `Transcription: ${liveTranscript}`);
  } else if (
    ev.type === "partial" &&
    ev.text !== "Refining transcript..." &&
    !ev.text.startsWith("Transcribing part ")
  ) {
    liveTranscript += ev.text;
    widget.dataset.liveTranscript = liveTranscript;
    widget.setAttribute("aria-label", `Live transcription: ${liveTranscript}`);
    if (streamTextToTarget) {
      const delta = ev.text;
      targetWriteQueue = targetWriteQueue.then(async () => {
        if (!streamTextToTarget) return;
        // The native side checks the field before each piece. Once it stops
        // (field changed, cursor moved, password field), the final text is
        // pasted without deleting what the user did.
        const write = await invoke<TargetWrite>("type_to_target", { text: delta });
        if (write.outcome !== "typed") {
          streamTextToTarget = false;
          logDebug(`live target typing stopped reason=${write.reason ?? "unknown"}`);
        }
      }).catch((err) => {
        logDebug(`live target write failed=${err instanceof Error ? err.message : String(err)}`);
      });
    }
  } else if (ev.type === "error") {
    sessionFailure = ev.text || "Transcription failed";
    await handleTranscriptionFailure(sessionFailure);
  } else if (ev.type === "warning") {
    showToast(ev.text, 8000);
  }
}

async function completeSession(sessionId: string) {
  // A cancel (or a new recording) while this waits takes the session over;
  // then nothing more is written, recorded or cleaned up here.
  const owned = () => currentSessionId === sessionId;
  if (finalTranscript) {
    lastTranscript = finalTranscript;
    localStorage.setItem("ow_last_transcript", lastTranscript);
    syncWidgetFrame();
    await targetWriteQueue;
    if (!owned()) return;
    if (!shouldPasteFinalTranscript) {
      await invoke("copy_to_clipboard", { text: finalTranscript });
      showToast("Transcript copied to clipboard", 3000);
    } else {
      const write = await invoke<TargetWrite>("insert_final_text", { text: finalTranscript });
      showToast(finalWriteNotice(write), write.outcome === "copied" ? 5000 : 2000);
    }
    if (!owned()) return;
    await finishCurrentRecording("complete", finalTranscript, "");
  } else if (sessionFailure) {
    await finishCurrentRecording("failed", "", sessionFailure);
  }
  if (!owned()) return;
  cleanup();
  widget.removeAttribute("data-live-transcript");
  widget.setAttribute("aria-label", "Click to start or stop transcription");
  setState("idle");
}

// A worker that stops during a transcription fails that request on its own.
// While recording no request is waiting, so this is the only notice.
async function handleWorkerExit(message: string) {
  logDebug(`worker exited state=${state} session=${currentSessionId ?? "none"}`);
  if (state !== "recording" || !currentSessionId) return;
  await handleTranscriptionFailure(message || "The transcription worker stopped");
  cleanup();
  setState("idle");
}

// Native end-to-end smoke hook for development builds. This deliberately uses
// the same Tauri commands, session events, and target-field streaming as a
// microphone recording; only the PCM source is deterministic test audio.
if (import.meta.env.DEV) {
  (window as typeof window & {
    __openwhisperRealtimeSmoke?: (chunks: string[], intervalMs?: number) => Promise<void>;
  }).__openwhisperRealtimeSmoke = async (chunks, intervalMs = 100) => {
    if (state !== "idle") throw new Error(`Cannot start smoke test while state=${state}`);
    // Only a Live session takes audio chunks; Standard reads the saved file.
    const settings = await currentSettings();
    if (!isLiveTranscription(settings.transcriptionModel)) {
      throw new Error("Choose Live transcription before the realtime smoke test");
    }
    await invoke("capture_paste_target");
    const result = await invoke<{ session_id: string }>("worker_start_session", {
      profileId: "realtime-smoke",
    });
    currentSessionId = result.session_id;
    finalTranscript = "";
    liveTranscript = "";
    streamTextToTarget = true;
    targetWriteQueue = Promise.resolve();
    setState("recording");
    for (const chunkBase64 of chunks) {
      // The test audio arrives as base64 text; the app sends raw bytes.
      const chunk = Uint8Array.from(atob(chunkBase64), (char) => char.charCodeAt(0));
      await sendAudioChunk(invoke, "worker_append_audio_chunk", currentSessionId, chunk);
      await new Promise((resolve) => window.setTimeout(resolve, intervalMs));
    }
    setState("transcribing");
    await invoke("worker_finalize_session_audio", {
      sessionId: currentSessionId,
      mimeType: PCM_MIME_TYPE,
    });
  };
}

// ---------------------------------------------------------------------------
// Toggle
// ---------------------------------------------------------------------------
async function toggleRecording() {
  // A second press while the microphone is still opening stops the recording
  // as soon as it starts (and a third press keeps it going after all).
  if (toggleRecordingStarting && (state === "idle" || state === "starting")) {
    toggleStopRequestedDuringStart = !toggleStopRequestedDuringStart;
    return;
  }
  if (state === "idle") {
    toggleRecordingStarting = true;
    toggleStopRequestedDuringStart = false;
    try {
      await startRecording();
      if (toggleStopRequestedDuringStart && state === "recording") {
        await stopRecording();
      }
    } finally {
      toggleRecordingStarting = false;
      toggleStopRequestedDuringStart = false;
    }
  } else if (state === "recording") {
    await stopRecording();
  }
}

async function handlePressToTalkStart() {
  logDebug(
    `pressToTalkStart event state=${state} held=${pressToTalkHeld} starting=${pressToTalkStarting}`,
  );
  pressToTalkHeld = true;
  if (state !== "idle" || pressToTalkStarting) return;

  pressToTalkStarting = true;
  try {
    await startRecording();
  } finally {
    pressToTalkStarting = false;
  }

  // If the user released the shortcut while startup was still in flight,
  // stop immediately once recording is actually active.
  if (!pressToTalkHeld && isRecording) {
    logDebug("pressToTalkStart detected release during startup");
    await stopRecording();
  }
}

async function handlePressToTalkStop() {
  logDebug(
    `pressToTalkStop event state=${state} held=${pressToTalkHeld} starting=${pressToTalkStarting}`,
  );
  pressToTalkHeld = false;
  if (state === "recording") {
    await stopRecording();
  }
}

btnMic.addEventListener("click", (e) => {
  e.stopPropagation();
  void toggleRecording();
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
  const bubbleWasVisible = settingsLoaded && !hasOpenaiApiKey && state === "idle";
  hasOpenaiApiKey = settings.hasOpenaiApiKey;
  settingsLoaded = true;
  const bubbleVisibilityChanged = bubbleWasVisible !== (!hasOpenaiApiKey && state === "idle");
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
  if ((bubbleVisibilityChanged || lastAppliedLayout?.visible !== getWindowLayout().visible) && lastAppliedLayout !== null) {
    void applyOverlayLayout();
  }
}

async function persistKeyInput() {
  if (keySaveInFlight || !keyInputDirty) return;

  keySaveInFlight = true;
  const key = menuKeyInput.value.trim();
  try {
    const updated = await invoke<AppSettings>("app_save_settings", { openaiApiKey: key });
    applySettingsUi(updated, updated.hasOpenaiApiKey ? "Key saved" : "Key cleared");
    showToast(updated.hasOpenaiApiKey ? "API key saved" : "API key cleared");
  } catch (err) {
    keyInputDirty = true;
    showToast(err instanceof Error ? err.message : String(err));
  } finally {
    keySaveInFlight = false;
  }
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
    showToast(err instanceof Error ? err.message : String(err));
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
  return appSettings ?? invoke<AppSettings>("app_get_settings");
}

menuShowIdlePillToggle.addEventListener("change", async () => {
  if (showIdlePillSaveInFlight) return;

  showIdlePillSaveInFlight = true;
  try {
    const updated = await invoke<AppSettings>("app_save_settings", {
      showIdlePill: menuShowIdlePillToggle.checked,
    });
    applySettingsUi(updated);
    showToast(updated.showIdlePill ? "Idle pill shown" : "Idle pill hidden");
  } catch (err) {
    showToast(err instanceof Error ? err.message : String(err));
    await loadSettings();
  } finally {
    showIdlePillSaveInFlight = false;
  }
});

menuHoldToTalkToggle.addEventListener("change", async () => {
  if (holdToTalkSaveInFlight) return;

  holdToTalkSaveInFlight = true;
  try {
    const updated = await invoke<AppSettings>("app_save_settings", {
      holdToTalk: menuHoldToTalkToggle.checked,
    });
    applySettingsUi(updated);
    showToast(updated.holdToTalk ? "Hold to talk enabled" : "Press to start and stop enabled");
  } catch (err) {
    showToast(err instanceof Error ? err.message : String(err));
    await loadSettings();
  } finally {
    holdToTalkSaveInFlight = false;
  }
});

menuModelSelect.addEventListener("change", async () => {
  if (modelSaveInFlight) return;
  syncModelHint();
  modelSaveInFlight = true;
  try {
    const updated = await invoke<AppSettings>("app_save_settings", {
      transcriptionModel: menuModelSelect.value,
    });
    applySettingsUi(updated);
    showToast("Transcription model saved");
  } catch (err) {
    showToast(err instanceof Error ? err.message : String(err));
    await loadSettings();
  } finally {
    modelSaveInFlight = false;
  }
});

async function ensureAutostartEnabled() {
  try {
    if (!(await isAutostartEnabled())) {
      await enableAutostart();
      logDebug("autostart enabled");
    }
  } catch (err) {
    logDebug(`autostart enable failed=${err instanceof Error ? err.message : String(err)}`);
  }
}

menuKeyInput.addEventListener("focus", () => {
  selectStoredKeyPreview();
  void revealStoredKeyForEdit();
});

menuKeyInput.addEventListener("click", () => {
  selectStoredKeyPreview();
  void revealStoredKeyForEdit();
});

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
    showToast(err instanceof Error ? err.message : String(err));
  }
});

// ---------------------------------------------------------------------------
// Settings (Transcription Prompt)
// ---------------------------------------------------------------------------
async function persistPromptInput() {
  if (promptSaveInFlight || !promptDirty) return;

  promptSaveInFlight = true;
  const prompt = menuPromptInput.value;
  try {
    const updated = await invoke<AppSettings>("app_save_settings", { transcriptionPrompt: prompt });
    promptDirty = false;
    applySettingsUi(updated, updated.hasOpenaiApiKey ? "Key configured" : "Not configured");
    showToast("Transcription prompt saved");
  } catch (err) {
    promptDirty = true;
    showToast(err instanceof Error ? err.message : String(err));
  } finally {
    promptSaveInFlight = false;
  }
}

menuPromptInput.addEventListener("input", () => {
  promptDirty = true;
});

menuPromptInput.addEventListener("blur", () => {
  if (!promptDirty) return;
  void persistPromptInput();
});

// ---------------------------------------------------------------------------
// Settings (Refine)
// ---------------------------------------------------------------------------
async function persistRefineInput() {
  if (refineSaveInFlight || !refineDirty) return;

  refineSaveInFlight = true;
  try {
    const updated = await invoke<AppSettings>("app_save_settings", {
      refineEnabled: menuRefineToggle.checked,
      refinePrompt: menuRefinePromptInput.value,
    });
    refineDirty = false;
    applySettingsUi(updated, updated.hasOpenaiApiKey ? "Key configured" : "Not configured");
  } catch (err) {
    refineDirty = true;
    showToast(err instanceof Error ? err.message : String(err));
  } finally {
    refineSaveInFlight = false;
  }
}

menuRefineToggle.addEventListener("change", () => {
  refineDirty = true;
  syncRefinePromptVisibility();
  void persistRefineInput();
});

menuRefinePromptInput.addEventListener("input", () => {
  refineDirty = true;
});

menuRefinePromptInput.addEventListener("blur", () => {
  if (!refineDirty) return;
  void persistRefineInput();
});

// ---------------------------------------------------------------------------
// Foreground app name: read when the menu opens, then sent by Rust only when
// the user moves to another program
// ---------------------------------------------------------------------------
function showForegroundApp(name: string | null) {
  menuTargetAppName.textContent = name ?? "Unknown";
}

async function refreshForegroundApp() {
  try {
    showForegroundApp(await invoke<string | null>("get_foreground_app"));
  } catch {
    showForegroundApp(null);
  }
}

void listen<string | null>("foreground-app-changed", (event) => showForegroundApp(event.payload));

// ---------------------------------------------------------------------------
// Tray / global shortcuts: listen for recording events emitted by Rust
// ---------------------------------------------------------------------------
void listen("toggle-recording", () => {
  logDebug("event toggle-recording");
  return void toggleRecording();
});
void listen("press-to-talk-start", () => {
  logDebug("event press-to-talk-start");
  return void handlePressToTalkStart();
});
void listen("press-to-talk-stop", () => {
  logDebug("event press-to-talk-stop");
  return void handlePressToTalkStop();
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

widget.dataset.state = state;
syncWidgetFrame();
syncApiKeyBubble();
void loadSettings().then(() => applyOverlayLayout(true));
if (import.meta.env.PROD) {
  void ensureAutostartEnabled();
}
void refreshMicDevices();
void invoke("worker_ping").catch(() => {});
// Ready before the first shortcut press; a failure is retried then.
void pcmCaptureContext().catch((error) => {
  logDebug(`audio capture warm-up failed=${errorMessage(error)}`);
});

if (navigator.mediaDevices?.addEventListener) {
  navigator.mediaDevices.addEventListener("devicechange", () => void refreshMicDevices());
}

// Installed builds update automatically. Development and automated UI runs
// must never replace or relaunch the executable under test.
if (import.meta.env.PROD) {
  setTimeout(() => void checkForUpdates(), 4000);
}
