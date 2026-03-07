import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  enable as enableAutostart,
  isEnabled as isAutostartEnabled,
} from "@tauri-apps/plugin-autostart";
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

let audioContext: AudioContext | null = null;
let analyser: AnalyserNode | null = null;
let micStream: MediaStream | null = null;
let volumeRafId: number | null = null;
let transcribeTimer: number | null = null;
let micDevices: MicDevice[] = [];
let selectedMicId = "default";
let menuOpen = false;

const IDLE_WIDGET_WIDTH = 88;
const IDLE_WIDGET_HEIGHT = 28;
const ACTIVE_WIDGET_WIDTH = 164;
const ACTIVE_WIDGET_HEIGHT = 34;
const TRANSCRIBE_TIMEOUT_MS = 20_000;
const MENU_WIDTH = 340;
const MENU_HEIGHT = 540;
const MENU_GAP = 12;

// ---------------------------------------------------------------------------
// DOM
// ---------------------------------------------------------------------------
const app = document.getElementById("app")!;
app.innerHTML = `
  <div id="widget-shell" data-tauri-drag-region>
    <div id="widget" data-tauri-drag-region>
      <button id="btn-mic" class="mic-btn" title="Click to toggle recording or hold Ctrl+Space to talk">
        <span class="mic-badge" aria-hidden="true">
          <svg class="icon-mic" viewBox="0 0 24 24" fill="currentColor">
            <path d="M12 15.25A3.25 3.25 0 0 0 15.25 12V6.25a3.25 3.25 0 1 0-6.5 0V12A3.25 3.25 0 0 0 12 15.25Z" />
            <path d="M6.5 11.5a.75.75 0 0 1 .75.75 4.75 4.75 0 0 0 9.5 0 .75.75 0 0 1 1.5 0 6.25 6.25 0 0 1-5.5 6.21v1.79h2a.75.75 0 0 1 0 1.5h-5.5a.75.75 0 0 1 0-1.5h2v-1.79a6.25 6.25 0 0 1-5.5-6.21.75.75 0 0 1 .75-.75Z" />
          </svg>
          <svg class="icon-stop" viewBox="0 0 24 24" fill="currentColor">
            <rect x="6.5" y="6.5" width="11" height="11" rx="3" />
          </svg>
          <svg class="icon-spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4">
            <path d="M12 2.75A9.25 9.25 0 0 1 21.25 12" stroke-linecap="round" />
          </svg>
        </span>
        <span class="meter" aria-hidden="true">
          <span class="meter-track"></span>
          <span id="meter-fill" class="meter-fill"></span>
        </span>
      </button>

      <button id="btn-copy" class="copy-btn" title="Copy last transcript to clipboard">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
          <rect x="9" y="9" width="11" height="11" rx="2" />
          <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
        </svg>
      </button>

      <button id="btn-dropdown" class="dropdown-btn" title="Select microphone and settings">
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
    </div>
  </div>

  <div id="toast" class="toast hidden"></div>
`;

const widget = document.getElementById("widget")!;
const btnMic = document.getElementById("btn-mic") as HTMLButtonElement;
const btnCopy = document.getElementById("btn-copy") as HTMLButtonElement;
const btnDropdown = document.getElementById("btn-dropdown") as HTMLButtonElement;
const micMenu = document.getElementById("mic-menu")!;
const micList = document.getElementById("mic-list")!;
const toast = document.getElementById("toast")!;
const meterFill = document.getElementById("meter-fill")!;
const menuKeyStatus = document.getElementById("menu-key-status")!;
const menuKeyInput = document.getElementById("menu-key-input") as HTMLInputElement;
const menuKeyLink = document.getElementById("menu-key-link") as HTMLAnchorElement;
const menuPromptInput = document.getElementById("menu-prompt-input") as HTMLTextAreaElement;
const menuTargetAppName = document.getElementById("menu-target-app-name")!;

let storedKeyPreview = "";
let showingStoredKeyPreview = false;
let keyInputDirty = false;
let keySaveInFlight = false;
let keyRevealInFlight = false;
let promptDirty = false;
let promptSaveInFlight = false;

setMeterLevel(0.12);

function logDebug(message: string) {
  const timestamp = new Date().toISOString();
  void invoke("debug_log", { message: `${timestamp} ${message}` }).catch(() => {});
}

function setMeterLevel(level: number) {
  const clamped = Math.max(0.08, Math.min(level, 1));
  meterFill.style.setProperty("--meter-level", clamped.toFixed(3));
}

function isWidgetExpanded() {
  return state !== "idle";
}

function getWidgetSize() {
  return isWidgetExpanded()
    ? { width: ACTIVE_WIDGET_WIDTH, height: ACTIVE_WIDGET_HEIGHT }
    : { width: IDLE_WIDGET_WIDTH, height: IDLE_WIDGET_HEIGHT };
}

function syncWidgetFrame() {
  const widgetSize = getWidgetSize();
  widget.style.setProperty("--widget-width", `${widgetSize.width}px`);
  widget.style.setProperty("--widget-height", `${widgetSize.height}px`);
  widget.dataset.expanded = String(isWidgetExpanded());
  widget.dataset.menuOpen = String(menuOpen);
  widget.dataset.hasLastTranscript = String(lastTranscript.length > 0);
}

function getWindowLayout() {
  const widgetSize = getWidgetSize();
  if (!menuOpen) {
    return widgetSize;
  }

  return {
    width: Math.max(widgetSize.width, MENU_WIDTH),
    height: widgetSize.height + MENU_GAP + MENU_HEIGHT,
  };
}

async function applyOverlayLayout() {
  syncWidgetFrame();
  const layout = getWindowLayout();
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
  stopAppPoll();
  micMenu.classList.add("hidden");
  await applyOverlayLayout();
}

btnDropdown.addEventListener("click", (e) => {
  e.stopPropagation();
  toggleMenu();
});

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
// Volume meter animation
// ---------------------------------------------------------------------------
function startVolumeLoop() {
  if (!analyser) return;
  const buf = new Uint8Array(analyser.fftSize);
  let smoothedLevel = 0.08;

  const tick = () => {
    analyser!.getByteTimeDomainData(buf);
    let sum = 0;
    for (let i = 0; i < buf.length; i++) {
      const v = (buf[i] - 128) / 128;
      sum += v * v;
    }
    const rms = Math.sqrt(sum / buf.length);
    const level = Math.min(rms / 0.16, 1);
    smoothedLevel =
      level > smoothedLevel
        ? smoothedLevel * 0.35 + level * 0.65
        : smoothedLevel * 0.72 + level * 0.28;
    setMeterLevel(0.08 + smoothedLevel * 0.92);
    volumeRafId = requestAnimationFrame(tick);
  };

  tick();
}

function stopVolumeLoop(restingLevel = 0.12) {
  if (volumeRafId !== null) {
    cancelAnimationFrame(volumeRafId);
    volumeRafId = null;
  }
  setMeterLevel(restingLevel);
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
// Recording
// ---------------------------------------------------------------------------
async function startRecording() {
  logDebug(
    `startRecording enter state=${state} held=${pressToTalkHeld} starting=${pressToTalkStarting}`,
  );
  if (state !== "idle") return;
  setState("recording");

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

    audioContext = new AudioContext();
    const source = audioContext.createMediaStreamSource(micStream);
    analyser = audioContext.createAnalyser();
    analyser.fftSize = 256;
    source.connect(analyser);
    startVolumeLoop();

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
  stopVolumeLoop(0.66);

  if (mediaRecorder && mediaRecorder.state !== "inactive") {
    await new Promise<void>((resolve) => {
      mediaRecorder!.addEventListener("stop", () => resolve(), { once: true });
      mediaRecorder!.stop();
    });
  }

  if (audioContext) {
    audioContext.close();
    audioContext = null;
    analyser = null;
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
  stopVolumeLoop();
  stopPolling();
  clearTranscribeTimeout();
  if (audioContext) {
    audioContext.close();
    audioContext = null;
    analyser = null;
  }
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
  if (!pressToTalkHeld && state === "recording") {
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

btnCopy.addEventListener("click", (e) => {
  e.stopPropagation();
  if (!lastTranscript) return;
  navigator.clipboard.writeText(lastTranscript).then(() => {
    showToast("Last transcript copied", 2000);
  }).catch(() => {
    showToast("Copy failed — try again");
  });
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

function applySettingsUi(settings: AppSettings, configuredLabel = "Key configured") {
  applyStoredKeyPreview(settings.openaiApiKeyPreview);
  menuKeyStatus.textContent = settings.hasOpenaiApiKey ? configuredLabel : "Not configured";
  menuKeyStatus.className = `menu-item ${settings.hasOpenaiApiKey ? "key-ok" : "key-missing"}`;
  if (!promptDirty) {
    menuPromptInput.value = settings.transcriptionPrompt ?? "";
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
    applySettingsUi({ hasOpenaiApiKey: false, openaiApiKeyPreview: null, transcriptionPrompt: "" });
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
setState("idle");
syncWidgetFrame();
void collapseOverlay();
void loadSettings();
void ensureAutostartEnabled();
void refreshMicDevices();
void invoke("worker_ping").catch(() => {});

if (navigator.mediaDevices?.addEventListener) {
  navigator.mediaDevices.addEventListener("devicechange", () => void refreshMicDevices());
}
