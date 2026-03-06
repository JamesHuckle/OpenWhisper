import { invoke } from "@tauri-apps/api/core";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { register } from "@tauri-apps/plugin-global-shortcut";
import "./styles.css";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------
type TranscriptEvent = { type: "partial" | "final"; text: string };
type PollResponse = { events: TranscriptEvent[]; done?: boolean };
type AppSettings = { openaiApiKey: string };

type WidgetState = "idle" | "recording" | "transcribing" | "error";

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------
let state: WidgetState = "idle";
let currentSessionId: string | null = null;
let pollTimer: number | null = null;
let finalTranscript = "";
let mediaRecorder: MediaRecorder | null = null;
let recordingMimeType = "audio/webm";
let isRecording = false;
const pendingChunkUploads = new Set<Promise<void>>();

let audioContext: AudioContext | null = null;
let analyser: AnalyserNode | null = null;
let micStream: MediaStream | null = null;
let volumeRafId: number | null = null;

const WIDGET_SIZE = 80;
const MENU_WIDTH = 340;
const MENU_HEIGHT = 420;

// ---------------------------------------------------------------------------
// DOM
// ---------------------------------------------------------------------------
const app = document.getElementById("app")!;
app.innerHTML = `
  <div id="widget" data-tauri-drag-region>
    <svg id="mic-ring" viewBox="0 0 80 80" class="mic-ring">
      <circle cx="40" cy="40" r="36" class="ring-bg" />
      <circle cx="40" cy="40" r="36" class="ring-vol" />
    </svg>

    <button id="btn-mic" class="mic-btn" title="Click to record (Ctrl+Shift+Space)">
      <svg class="icon-mic" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <rect x="9" y="1" width="6" height="12" rx="3" />
        <path d="M19 10v1a7 7 0 0 1-14 0v-1" />
        <line x1="12" y1="19" x2="12" y2="23" />
        <line x1="8" y1="23" x2="16" y2="23" />
      </svg>
      <svg class="icon-stop" viewBox="0 0 24 24" fill="currentColor">
        <rect x="6" y="6" width="12" height="12" rx="2" />
      </svg>
      <svg class="icon-spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
        <path d="M12 2a10 10 0 0 1 10 10" stroke-linecap="round" />
      </svg>
    </button>

    <button id="btn-dropdown" class="dropdown-btn" title="Select microphone">
      <svg viewBox="0 0 12 12" fill="currentColor"><path d="M2 4l4 4 4-4z"/></svg>
    </button>

    <div id="mic-menu" class="mic-menu hidden">
      <div class="menu-section">Microphone</div>
      <div id="mic-list"></div>
      <div class="menu-divider"></div>
      <div class="menu-section">OpenAI API Key</div>
      <div class="menu-item" id="menu-key-status">Not configured</div>
      <input id="menu-key-input" class="menu-key-input" type="password" placeholder="sk-..." autocomplete="off" />
      <button id="menu-key-save" class="menu-key-save">Save</button>
    </div>
  </div>

  <div id="toast" class="toast hidden"></div>
`;

const widget = document.getElementById("widget")!;
const btnMic = document.getElementById("btn-mic") as HTMLButtonElement;
const btnDropdown = document.getElementById("btn-dropdown") as HTMLButtonElement;
const micMenu = document.getElementById("mic-menu")!;
const micList = document.getElementById("mic-list")!;
const toast = document.getElementById("toast")!;
const ringVol = widget.querySelector<SVGCircleElement>(".ring-vol")!;
const menuKeyStatus = document.getElementById("menu-key-status")!;
const menuKeyInput = document.getElementById("menu-key-input") as HTMLInputElement;
const menuKeySave = document.getElementById("menu-key-save") as HTMLButtonElement;

const RING_CIRCUMFERENCE = 2 * Math.PI * 36;
ringVol.style.strokeDasharray = `${RING_CIRCUMFERENCE}`;
ringVol.style.strokeDashoffset = `${RING_CIRCUMFERENCE}`;

// ---------------------------------------------------------------------------
// Mic devices
// ---------------------------------------------------------------------------
type MicDevice = { id: string; label: string };
let micDevices: MicDevice[] = [];
let selectedMicId = "default";

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
  micMenu.classList.toggle("hidden");
  if (!micMenu.classList.contains("hidden")) {
    resizeForMenu(true);
  } else {
    resizeForMenu(false);
  }
}

function closeMenu() {
  micMenu.classList.add("hidden");
  resizeForMenu(false);
}

async function resizeForMenu(open: boolean) {
  const win = getCurrentWindow();
  if (open) {
    await win.setSize(new LogicalSize(MENU_WIDTH, MENU_HEIGHT));
  } else {
    await win.setSize(new LogicalSize(WIDGET_SIZE, WIDGET_SIZE));
  }
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
  state = next;
  widget.dataset.state = next;
}

// ---------------------------------------------------------------------------
// Volume ring animation
// ---------------------------------------------------------------------------
function startVolumeLoop() {
  if (!analyser) return;
  const buf = new Uint8Array(analyser.fftSize);
  const tick = () => {
    analyser!.getByteTimeDomainData(buf);
    let sum = 0;
    for (let i = 0; i < buf.length; i++) {
      const v = (buf[i] - 128) / 128;
      sum += v * v;
    }
    const rms = Math.sqrt(sum / buf.length);
    const level = Math.min(rms / 0.35, 1);
    const offset = RING_CIRCUMFERENCE * (1 - level);
    ringVol.style.strokeDashoffset = `${offset}`;
    volumeRafId = requestAnimationFrame(tick);
  };
  tick();
}

function stopVolumeLoop() {
  if (volumeRafId !== null) {
    cancelAnimationFrame(volumeRafId);
    volumeRafId = null;
  }
  ringVol.style.strokeDashoffset = `${RING_CIRCUMFERENCE}`;
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
  if (state !== "idle") return;
  setState("recording");

  try {
    const settings = await invoke<AppSettings>("app_get_settings");
    if (!settings.openaiApiKey) {
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

    const result = await invoke<{ session_id: string }>("worker_start_session", { profileId: "default" });
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
  } catch (err) {
    showToast(err instanceof Error ? err.message : String(err));
    cleanup();
    setState("idle");
  }
}

async function stopRecording() {
  if (state !== "recording" || !currentSessionId) return;
  setState("transcribing");
  stopVolumeLoop();

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
  } catch (err) {
    showToast(err instanceof Error ? err.message : String(err));
  }
}

function cleanup() {
  stopVolumeLoop();
  stopPolling();
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
    const resp = await invoke<PollResponse>("worker_poll_session_events", { sessionId: currentSessionId });
    for (const ev of resp.events ?? []) {
      if (ev.type === "final") {
        finalTranscript = `${finalTranscript} ${ev.text}`.trim();
      }
    }
    if (resp.done) {
      stopPolling();
      if (finalTranscript) {
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

btnMic.addEventListener("click", (e) => {
  e.stopPropagation();
  void toggleRecording();
});

// ---------------------------------------------------------------------------
// Settings (API key)
// ---------------------------------------------------------------------------
async function loadSettings() {
  try {
    const s = await invoke<AppSettings>("app_get_settings");
    menuKeyInput.value = s.openaiApiKey || "";
    menuKeyStatus.textContent = s.openaiApiKey ? "Key configured" : "Not configured";
    menuKeyStatus.className = `menu-item ${s.openaiApiKey ? "key-ok" : "key-missing"}`;
  } catch {
    menuKeyStatus.textContent = "Not configured";
  }
}

menuKeySave.addEventListener("click", async () => {
  const key = menuKeyInput.value.trim();
  try {
    await invoke<AppSettings>("app_save_settings", { openaiApiKey: key });
    menuKeyStatus.textContent = key ? "Key saved" : "Key cleared";
    menuKeyStatus.className = `menu-item ${key ? "key-ok" : "key-missing"}`;
    showToast(key ? "API key saved" : "API key cleared");
  } catch (err) {
    showToast(err instanceof Error ? err.message : String(err));
  }
});

// ---------------------------------------------------------------------------
// Global shortcut: Ctrl+Shift+Space
// ---------------------------------------------------------------------------
async function registerShortcut() {
  const win = getCurrentWindow();
  try {
    await register("Control+Shift+Space", (event) => {
      if (event.state === "Pressed") {
        void invoke("save_target_window").then(async () => {
          await win.show();
          await win.setFocus();
          await toggleRecording();
        });
      }
    });
  } catch (err) {
    console.warn("Failed to register Ctrl+Shift+Space shortcut:", err);
  }
}

// ---------------------------------------------------------------------------
// Tray: listen for toggle-recording emitted by the Rust tray menu
// ---------------------------------------------------------------------------
void listen("toggle-recording", () => void toggleRecording());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------
setState("idle");
void loadSettings();
void refreshMicDevices();
void registerShortcut();

if (navigator.mediaDevices?.addEventListener) {
  navigator.mediaDevices.addEventListener("devicechange", () => void refreshMicDevices());
}
