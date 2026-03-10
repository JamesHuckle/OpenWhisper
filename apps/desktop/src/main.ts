import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  enable as enableAutostart,
  isEnabled as isAutostartEnabled,
} from "@tauri-apps/plugin-autostart";
import { continuousVisualizer } from "sound-visualizer";
import "./styles.css";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------
type TranscriptEvent = { type: "partial" | "final" | "error"; text: string };
type PollResponse = { events: TranscriptEvent[]; done?: boolean };
type AppSettings = {
  hasOpenaiApiKey: boolean;
  openaiApiKeyPreview: string | null;
  transcriptionPrompt: string;
  refineEnabled: boolean;
  refinePrompt: string;
};
type WidgetState = "idle" | "recording" | "transcribing" | "error";
type MicDevice = { id: string; label: string };

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------
let state: WidgetState = "idle";
let currentSessionId: string | null = null;
let pollTimer: number | null = null;
let finalTranscript = "";
let lastTranscript = localStorage.getItem("ow_last_transcript") ?? "";
let mediaRecorder: MediaRecorder | null = null;
let recordingMimeType = "audio/webm";
let isRecording = false;
let pressToTalkHeld = false;
let pressToTalkStarting = false;
const pendingChunkUploads = new Set<Promise<void>>();

let micStream: MediaStream | null = null;
let soundVisualizer: { start: () => void; stop: () => void; reset: () => void } | null = null;
let volumeMonitorFrameId: number | null = null;
let volumeMonitorDisconnect: (() => void) | null = null;
const SILENT_THRESHOLD = 14;
let transcribeTimer: number | null = null;
let micDevices: MicDevice[] = [];
let selectedMicId = "default";
let menuOpen = false;

const COLLAPSED_WIDTH = 38;
const COLLAPSED_HEIGHT = 14;
const EXPANDED_WIDTH = 100;
const EXPANDED_HEIGHT = 28;
const TRANSCRIBE_TIMEOUT_MS = 20_000;
const MENU_GAP = 12;

// ---------------------------------------------------------------------------
// DOM
// ---------------------------------------------------------------------------
const app = document.getElementById("app")!;
app.innerHTML = `
  <div id="widget-shell">
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

      <button id="btn-dropdown" class="dropdown-btn" type="button" title="Select microphone and settings">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">
          <path d="M7.5 10.5 12 15 16.5 10.5" />
        </svg>
      </button>
    </div>

    <div id="mic-menu" class="mic-menu hidden">
      <div class="menu-section">Microphone</div>
      <div id="mic-list"></div>
      <div class="menu-divider"></div>
      <div class="menu-section">OpenAI API Key</div>
      <div class="menu-item" id="menu-key-status">Not configured</div>
      <input id="menu-key-input" class="menu-key-input" type="password" placeholder="sk-..." autocomplete="off" spellcheck="false" />
      <a id="menu-key-link" class="menu-link" href="https://platform.openai.com/api-keys">
        <span class="menu-link-bold">Create or manage</span> keys at platform.openai.com/api-keys
      </a>
      <div class="menu-divider"></div>
      <div class="menu-section">Transcription Prompt</div>
      <div id="menu-target-app" class="menu-target-app">
        <span class="target-app-dot"></span>
        <span id="menu-target-app-name">Detecting…</span>
      </div>
      <textarea id="menu-prompt-input" class="menu-prompt-input" rows="3" placeholder="e.g. Glossary: @README.md, @package.json, Daniël" spellcheck="false"></textarea>
      <div class="menu-prompt-hint">Style guide &amp; vocabulary for the transcription model (224 token limit)</div>
      <div class="menu-divider"></div>
      <div class="menu-toggle-row">
        <span class="menu-section menu-section-inline">Refine</span>
        <label class="toggle-switch">
          <input id="menu-refine-toggle" type="checkbox" />
          <span class="toggle-track"></span>
          <span class="toggle-thumb"></span>
        </label>
      </div>
      <textarea id="menu-refine-prompt-input" class="menu-prompt-input" rows="3" placeholder="Leave blank for default: strip fillers, fix sentences, add lists when enumerated" spellcheck="false"></textarea>
      <div class="menu-prompt-hint">Custom instructions for the refinement pass (gpt-5-nano)</div>
    </div>
  </div>

  <div id="toast" class="toast hidden"></div>
`;

const widget = document.getElementById("widget")!;
const meterCanvas = document.getElementById("meter-canvas") as HTMLCanvasElement;
const waveBars = document.querySelectorAll<HTMLElement>(".wave-bar");
const btnMic = document.getElementById("btn-mic") as HTMLButtonElement;
const btnDropdown = document.getElementById("btn-dropdown") as HTMLButtonElement;
const micMenu = document.getElementById("mic-menu")!;
const micList = document.getElementById("mic-list")!;
const toast = document.getElementById("toast")!;
const menuKeyStatus = document.getElementById("menu-key-status")!;
const menuKeyInput = document.getElementById("menu-key-input") as HTMLInputElement;
const menuKeyLink = document.getElementById("menu-key-link") as HTMLAnchorElement;
const menuPromptInput = document.getElementById("menu-prompt-input") as HTMLTextAreaElement;
const menuRefineToggle = document.getElementById("menu-refine-toggle") as HTMLInputElement;
const menuRefinePromptInput = document.getElementById("menu-refine-prompt-input") as HTMLTextAreaElement;
const menuTargetAppName = document.getElementById("menu-target-app-name")!;

let storedKeyPreview = "";
let showingStoredKeyPreview = false;
let keyInputDirty = false;
let keySaveInFlight = false;
let keyRevealInFlight = false;
let promptDirty = false;
let promptSaveInFlight = false;
let refineDirty = false;
let refineSaveInFlight = false;
let widgetHovered = false;
let expandTimeoutId: number | null = null;
let collapseTimeoutId: number | null = null;
let lastAppliedLayout: { width: number; height: number } | null = null;

function logDebug(message: string) {
  const timestamp = new Date().toISOString();
  void invoke("debug_log", { message: `${timestamp} ${message}` }).catch(() => {});
}

function isWidgetExpanded() {
  return widgetHovered || menuOpen || state !== "idle";
}

function getWidgetSize() {
  if (isWidgetExpanded()) {
    return { width: EXPANDED_WIDTH, height: EXPANDED_HEIGHT };
  }
  return { width: COLLAPSED_WIDTH, height: COLLAPSED_HEIGHT };
}

function syncWidgetFrame() {
  const widgetSize = getWidgetSize();
  widget.style.setProperty("--widget-width", `${widgetSize.width}px`);
  widget.style.setProperty("--widget-height", `${widgetSize.height}px`);
  widget.dataset.expanded = String(isWidgetExpanded());
  widget.dataset.hovered = String(widgetHovered);
  widget.dataset.menuOpen = String(menuOpen);
  widget.dataset.hasLastTranscript = String(lastTranscript.length > 0);
}

function setWidgetHovered(next: boolean) {
  if (widgetHovered === next) return;
  widgetHovered = next;
  widget.dataset.hovered = String(next);
  void applyOverlayLayout();
}

function getWindowLayout() {
  const widgetSize = getWidgetSize();
  if (!menuOpen || micMenu.classList.contains("hidden")) {
    return widgetSize;
  }

  const menuRect = micMenu.getBoundingClientRect();
  return {
    width: Math.max(widgetSize.width, Math.ceil(menuRect.width)),
    height: widgetSize.height + MENU_GAP + Math.ceil(menuRect.height),
  };
}

async function applyOverlayLayout(force = false) {
  syncWidgetFrame();
  const layout = getWindowLayout();
  if (
    !force &&
    lastAppliedLayout?.width === layout.width &&
    lastAppliedLayout?.height === layout.height
  ) {
    return;
  }

  lastAppliedLayout = layout;
  await invoke("overlay_apply_layout", { width: layout.width, height: layout.height });
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
  startAppPoll();
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
  stopAppPoll();
  micMenu.classList.add("hidden");
  await applyOverlayLayout();
}

btnDropdown.addEventListener("click", (e) => {
  e.stopPropagation();
  toggleMenu();
});

function handleWidgetClick(e: MouseEvent) {
  if (state === "recording" || state === "transcribing") {
    e.stopPropagation();
    e.preventDefault();
    if (state === "recording") {
      void stopRecording();
    } else {
      cleanup();
      setState("idle");
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
        cleanup();
        setState("idle");
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
  setWidgetHovered(true);
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
  }
  setWidgetHovered(false);
}

widget.addEventListener("pointerenter", handleWidgetPointerEnter);
widget.addEventListener("pointerleave", handleWidgetPointerLeave);

document.addEventListener("click", (e) => {
  if (!micMenu.contains(e.target as Node) && e.target !== btnDropdown) {
    if (!micMenu.classList.contains("hidden")) closeMenu();
  }
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
  widget.dataset.state = next;
  void applyOverlayLayout();
}

// ---------------------------------------------------------------------------
// Meter — sound-visualizer (recording) + idle bars (transcribing)
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
// Transcription timeout
// ---------------------------------------------------------------------------
function startTranscribeTimeout() {
  clearTranscribeTimeout();
  transcribeTimer = window.setTimeout(() => {
    if (state === "transcribing") {
      showToast("Transcription timed out - try again");
      cleanup();
      setState("idle");
    }
  }, TRANSCRIBE_TIMEOUT_MS);
}

function clearTranscribeTimeout() {
  if (transcribeTimer !== null) {
    clearTimeout(transcribeTimer);
    transcribeTimer = null;
  }
}

// ---------------------------------------------------------------------------
// Base64 helper
// ---------------------------------------------------------------------------
function toBase64(buf: ArrayBuffer): string {
  const bytes = new Uint8Array(buf);
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

// ---------------------------------------------------------------------------
// Volume monitor — show wobbly line when mic is silent during recording
// ---------------------------------------------------------------------------
function startVolumeMonitor(stream: MediaStream) {
  const ctx = new AudioContext();
  const analyser = ctx.createAnalyser();
  analyser.fftSize = 256;
  const source = ctx.createMediaStreamSource(stream);
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
    source.disconnect();
    analyser.disconnect();
    ctx.close();
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
async function startRecording() {
  logDebug(
    `startRecording enter state=${state} held=${pressToTalkHeld} starting=${pressToTalkStarting}`,
  );
  if (state !== "idle") return;
  setState("recording");
  widget.dataset.silent = "true";

  try {
    const settings = await invoke<AppSettings>("app_get_settings");
    if (!settings.hasOpenaiApiKey) {
      showToast("Set your OpenAI API key first (click arrow)");
      setState("idle");
      return;
    }

    const constraints: MediaStreamConstraints = {
      audio: selectedMicId === "default" ? true : { deviceId: { exact: selectedMicId } },
    };
    micStream = await navigator.mediaDevices.getUserMedia(constraints);

    soundVisualizer = continuousVisualizer(micStream, meterCanvas, {
      strokeColor: "#9ef0c9",
      rectWidth: 3,
      slices: 20,
    });
    soundVisualizer.start();
    startVolumeMonitor(micStream);

    const result = await invoke<{ session_id: string }>("worker_start_session", {
      profileId: "default",
    });
    currentSessionId = result.session_id;
    finalTranscript = "";
    startPolling();

    const mimeType = MediaRecorder.isTypeSupported("audio/webm;codecs=opus")
      ? "audio/webm;codecs=opus"
      : "audio/webm";
    recordingMimeType = "audio/webm";
    mediaRecorder = new MediaRecorder(micStream, { mimeType });

    mediaRecorder.ondataavailable = async (e: BlobEvent) => {
      if (!currentSessionId || e.data.size === 0) return;
      const p = (async () => {
        const chunkBase64 = toBase64(await e.data.arrayBuffer());
        await invoke("worker_append_audio_chunk", { sessionId: currentSessionId, chunkBase64 });
      })();
      pendingChunkUploads.add(p);
      p.finally(() => pendingChunkUploads.delete(p));
    };

    mediaRecorder.onstop = () => {
      micStream?.getTracks().forEach((t) => t.stop());
      isRecording = false;
    };

    mediaRecorder.start(800);
    isRecording = true;
    logDebug(`startRecording ready session=${currentSessionId ?? "none"} held=${pressToTalkHeld}`);
  } catch (err) {
    logDebug(`startRecording error=${err instanceof Error ? err.message : String(err)}`);
    showToast(err instanceof Error ? err.message : String(err));
    cleanup();
    setState("idle");
  }
}

async function stopRecording() {
  logDebug(`stopRecording enter state=${state} session=${currentSessionId ?? "none"}`);
  if (state !== "recording" || !currentSessionId) return;
  setState("transcribing");
  startTranscribeTimeout();
  if (soundVisualizer) {
    soundVisualizer.stop();
    soundVisualizer = null;
  }
  stopVolumeMonitor();
  setWaveBarLevels(Array(VOLUME_BAR_COUNT).fill(0.5));

  if (mediaRecorder && mediaRecorder.state !== "inactive") {
    await new Promise<void>((resolve) => {
      mediaRecorder!.addEventListener("stop", () => resolve(), { once: true });
      mediaRecorder!.stop();
    });
  }

  await Promise.all(Array.from(pendingChunkUploads));

  try {
    await invoke("worker_finalize_session_audio", {
      sessionId: currentSessionId,
      mimeType: recordingMimeType,
    });
    logDebug(`stopRecording finalized session=${currentSessionId}`);
  } catch (err) {
    logDebug(`stopRecording error=${err instanceof Error ? err.message : String(err)}`);
    showToast(err instanceof Error ? err.message : String(err));
    cleanup();
    setState("idle");
  }
}

function cleanup() {
  logDebug(`cleanup state=${state} session=${currentSessionId ?? "none"}`);
  stopPolling();
  clearTranscribeTimeout();
  if (soundVisualizer) {
    soundVisualizer.stop();
    soundVisualizer = null;
  }
  stopVolumeMonitor();
  if (mediaRecorder && mediaRecorder.state !== "inactive") {
    mediaRecorder.stop();
  }
  micStream?.getTracks().forEach((t) => t.stop());
  mediaRecorder = null;
  micStream = null;
  currentSessionId = null;
}

// ---------------------------------------------------------------------------
// Polling
// ---------------------------------------------------------------------------
function startPolling() {
  stopPolling();
  pollTimer = window.setInterval(() => void pollOnce(), 350);
}

function stopPolling() {
  if (pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

async function pollOnce() {
  if (!currentSessionId) return;
  try {
    const resp = await invoke<PollResponse>("worker_poll_session_events", {
      sessionId: currentSessionId,
    });
    for (const ev of resp.events ?? []) {
      if (ev.type === "final") {
        finalTranscript = `${finalTranscript} ${ev.text}`.trim();
      } else if (ev.type === "error") {
        showToast(ev.text || "Transcription failed", 3000);
      }
    }
    if (resp.done) {
      clearTranscribeTimeout();
      stopPolling();
      if (finalTranscript) {
        lastTranscript = finalTranscript;
        localStorage.setItem("ow_last_transcript", lastTranscript);
        syncWidgetFrame();
        await invoke("paste_to_target", { text: finalTranscript });
        showToast("Transcribed and pasted", 2000);
      }
      cleanup();
      setState("idle");
    }
  } catch {
    // transient poll errors are non-fatal
  }
}

// ---------------------------------------------------------------------------
// Toggle
// ---------------------------------------------------------------------------
async function toggleRecording() {
  if (state === "idle") {
    await startRecording();
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
  if (pressToTalkStarting) return;
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

function applySettingsUi(settings: AppSettings, configuredLabel = "Key configured") {
  applyStoredKeyPreview(settings.openaiApiKeyPreview);
  menuKeyStatus.textContent = settings.hasOpenaiApiKey ? configuredLabel : "Not configured";
  menuKeyStatus.className = `menu-item ${settings.hasOpenaiApiKey ? "key-ok" : "key-missing"}`;
  if (!promptDirty) {
    menuPromptInput.value = settings.transcriptionPrompt ?? "";
  }
  if (!refineDirty) {
    menuRefineToggle.checked = settings.refineEnabled ?? true;
    menuRefinePromptInput.value = settings.refinePrompt ?? "";
    syncRefinePromptVisibility();
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
    applySettingsUi({ hasOpenaiApiKey: false, openaiApiKeyPreview: null, transcriptionPrompt: "", refineEnabled: true, refinePrompt: "" });
  }
}

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
// Foreground app detection (polls while menu is open)
// ---------------------------------------------------------------------------
let appPollTimer: number | null = null;

async function pollForegroundApp() {
  try {
    const name = await invoke<string | null>("get_foreground_app");
    menuTargetAppName.textContent = name ?? "Unknown";
  } catch {
    menuTargetAppName.textContent = "Unknown";
  }
}

function startAppPoll() {
  void pollForegroundApp();
  stopAppPoll();
  appPollTimer = window.setInterval(() => void pollForegroundApp(), 500);
}

function stopAppPoll() {
  if (appPollTimer !== null) {
    clearInterval(appPollTimer);
    appPollTimer = null;
  }
}

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
void listen("overlay-revealed", () => void collapseOverlay());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------
widget.dataset.state = state;
syncWidgetFrame();
void applyOverlayLayout(true);
void loadSettings();
void ensureAutostartEnabled();
void refreshMicDevices();
void invoke("worker_ping").catch(() => {});

if (navigator.mediaDevices?.addEventListener) {
  navigator.mediaDevices.addEventListener("devicechange", () => void refreshMicDevices());
}
