import { invoke } from "@tauri-apps/api/core";
import "./styles.css";

type ModelsResponse = { models: string[] };
type TranscriptEvent = { type: "partial" | "final"; text: string };
type PollResponse = { events: TranscriptEvent[]; done?: boolean };
type AudioDeviceOption = { id: string; label: string };
type AppSettings = { openaiApiKey: string };

const app = document.querySelector<HTMLDivElement>("#app");
if (!app) {
  throw new Error("App root not found");
}

app.innerHTML = `
  <h1>OpenWhisper</h1>
  <div class="card">
    <h3>OpenAI API key</h3>
    <p class="muted">Paste your key once. It is stored locally on this device.</p>
    <div class="settings-row">
      <input id="openai-key" class="text-input" type="password" placeholder="sk-..." autocomplete="off" />
      <button id="btn-save-key">Save key</button>
    </div>
    <pre id="settings-output">Key not configured.</pre>
  </div>
  <div class="card">
    <h3>Worker health</h3>
    <button id="btn-ping">Ping worker</button>
    <button id="btn-models" class="secondary">List models</button>
    <pre id="models-output">No data yet.</pre>
  </div>
  <div class="card">
    <h3>Session smoke test</h3>
    <button id="btn-start">Start session</button>
    <button id="btn-record" class="secondary">Start mic</button>
    <button id="btn-finalize" class="secondary">Stop + transcribe</button>
    <button id="btn-stop" class="secondary">Stop session</button>
    <div class="mic-controls">
      <label for="mic-device">Mic</label>
      <select id="mic-device" class="mic-select">
        <option value="default">Default microphone</option>
      </select>
    </div>
    <pre id="session-output">No session started.</pre>
  </div>
  <div class="card">
    <h3>Live transcript</h3>
    <pre id="transcript-output">No transcript yet.</pre>
  </div>
`;

const modelsOutput = document.querySelector<HTMLPreElement>("#models-output");
const sessionOutput = document.querySelector<HTMLPreElement>("#session-output");
const transcriptOutput = document.querySelector<HTMLPreElement>("#transcript-output");
const settingsOutput = document.querySelector<HTMLPreElement>("#settings-output");
const apiKeyInput = document.querySelector<HTMLInputElement>("#openai-key");

if (!modelsOutput || !sessionOutput || !transcriptOutput || !settingsOutput || !apiKeyInput) {
  throw new Error("Missing output element");
}

let currentSessionId: string | null = null;
let pollTimer: number | null = null;
let finalTranscript = "";
let mediaRecorder: MediaRecorder | null = null;
let recordingMimeType = "audio/webm";
let isRecording = false;
const pendingChunkUploads = new Set<Promise<void>>();
let availableMicDevices: AudioDeviceOption[] = [];

function normalizeMicError(error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  const lower = message.toLowerCase();
  if (lower.includes("notallowederror") || lower.includes("permission denied")) {
    return {
      error: "Microphone permission denied or blocked by platform",
      details:
        "Allow microphone access for this app/window. On WSL2/WSLg this can also mean platform-level mic bridging is unavailable.",
    };
  }
  if (lower.includes("notfounderror")) {
    return {
      error: "No microphone input device found",
      details: "Connect a microphone and ensure the OS can see it.",
    };
  }
  if (lower.includes("notreadableerror")) {
    return {
      error: "Microphone device is busy or unavailable",
      details: "Close other apps using the mic and retry.",
    };
  }
  return { error: message };
}

function getMicSelect() {
  return document.querySelector<HTMLSelectElement>("#mic-device");
}

function selectedMicDeviceId() {
  const select = getMicSelect();
  return select?.value || "default";
}

function selectedMicLabel() {
  const id = selectedMicDeviceId();
  if (id === "default") {
    return "Default microphone";
  }
  return availableMicDevices.find((device) => device.id === id)?.label ?? "Selected microphone";
}

function renderMicDevices() {
  const select = getMicSelect();
  if (!select) {
    return;
  }
  const previous = select.value;
  const options: AudioDeviceOption[] = [
    { id: "default", label: "Default microphone" },
    ...availableMicDevices,
  ];
  select.innerHTML = "";
  for (const option of options) {
    const el = document.createElement("option");
    el.value = option.id;
    el.textContent = option.label;
    select.appendChild(el);
  }
  if (options.some((option) => option.id === previous)) {
    select.value = previous;
  }
}

async function loadMicDevices() {
  const devices = await navigator.mediaDevices.enumerateDevices();
  const audioInputs = devices.filter((device) => device.kind === "audioinput");
  availableMicDevices = audioInputs.map((device, index) => ({
    id: device.deviceId,
    label: device.label || `Microphone ${index + 1}`,
  }));
  renderMicDevices();
}

async function ensureMicPermissionAndDevices() {
  const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
  for (const track of stream.getTracks()) {
    track.stop();
  }
  await loadMicDevices();
}

async function setModelsOutput(value: unknown) {
  modelsOutput.textContent = JSON.stringify(value, null, 2);
}

async function setSessionOutput(value: unknown) {
  sessionOutput.textContent = JSON.stringify(value, null, 2);
}

async function setTranscriptOutput(value: string) {
  transcriptOutput.textContent = value;
}

async function setSettingsOutput(value: unknown) {
  settingsOutput.textContent = JSON.stringify(value, null, 2);
}

async function loadAppSettings() {
  const settings = await invoke<AppSettings>("app_get_settings");
  apiKeyInput.value = settings.openaiApiKey || "";
  await setSettingsOutput({
    hasOpenAiApiKey: Boolean(settings.openaiApiKey),
    status: settings.openaiApiKey ? "configured" : "missing",
  });
}

function stopPolling() {
  if (pollTimer !== null) {
    window.clearInterval(pollTimer);
    pollTimer = null;
  }
}

async function pollOnce() {
  if (!currentSessionId) {
    return;
  }
  const response = await invoke<PollResponse>("worker_poll_session_events", { sessionId: currentSessionId });
  const events = response.events ?? [];
  for (const event of events) {
    if (event.type === "partial") {
      await setTranscriptOutput(`${finalTranscript}\n${event.text}`.trim());
    }
    if (event.type === "final") {
      finalTranscript = `${finalTranscript} ${event.text}`.trim();
      await setTranscriptOutput(finalTranscript);
    }
  }
  if (response.done) {
    stopPolling();
  }
}

function startPolling() {
  stopPolling();
  pollTimer = window.setInterval(() => {
    void pollOnce();
  }, 350);
}

function toBase64(arrayBuffer: ArrayBuffer): string {
  const bytes = new Uint8Array(arrayBuffer);
  let binary = "";
  const chunkSize = 0x8000;
  for (let i = 0; i < bytes.length; i += chunkSize) {
    const slice = bytes.subarray(i, i + chunkSize);
    binary += String.fromCharCode(...slice);
  }
  return btoa(binary);
}

async function startMicCapture() {
  if (!currentSessionId) {
    await setSessionOutput({ error: "Start a session first" });
    return;
  }
  if (isRecording) {
    await setSessionOutput({ info: "Already recording" });
    return;
  }

  await ensureMicPermissionAndDevices();
  const requestedDeviceId = selectedMicDeviceId();
  const stream = await navigator.mediaDevices.getUserMedia({
    audio:
      requestedDeviceId === "default"
        ? true
        : {
            deviceId: { exact: requestedDeviceId },
          },
  });
  const mimeType = MediaRecorder.isTypeSupported("audio/webm;codecs=opus")
    ? "audio/webm;codecs=opus"
    : "audio/webm";
  recordingMimeType = "audio/webm";
  mediaRecorder = new MediaRecorder(stream, { mimeType });

  mediaRecorder.ondataavailable = async (event: BlobEvent) => {
    if (!currentSessionId || event.data.size === 0) {
      return;
    }
    const uploadPromise = (async () => {
      const buffer = await event.data.arrayBuffer();
      const chunkBase64 = toBase64(buffer);
      await invoke("worker_append_audio_chunk", {
        sessionId: currentSessionId,
        chunkBase64,
      });
    })();
    pendingChunkUploads.add(uploadPromise);
    await uploadPromise.finally(() => pendingChunkUploads.delete(uploadPromise));
  };

  mediaRecorder.onstop = () => {
    for (const track of stream.getTracks()) {
      track.stop();
    }
    isRecording = false;
  };

  mediaRecorder.start(800);
  isRecording = true;
  await setSessionOutput({
    status: "recording",
    session_id: currentSessionId,
    microphone: selectedMicLabel(),
  });
}

async function stopMicAndFinalize() {
  if (!currentSessionId) {
    await setSessionOutput({ error: "No active session" });
    return;
  }
  if (!mediaRecorder) {
    await setSessionOutput({ error: "Start mic before transcribing" });
    return;
  }
  if (mediaRecorder.state !== "inactive") {
    await new Promise<void>((resolve) => {
      mediaRecorder?.addEventListener(
        "stop",
        () => {
          resolve();
        },
        { once: true },
      );
      mediaRecorder?.stop();
    });
  }
  await Promise.all(Array.from(pendingChunkUploads));
  const result = await invoke("worker_finalize_session_audio", {
    sessionId: currentSessionId,
    mimeType: recordingMimeType,
  });
  await setSessionOutput({ status: "finalized", result });
}

document.querySelector<HTMLButtonElement>("#btn-ping")?.addEventListener("click", async () => {
  try {
    const result = await invoke("worker_ping");
    await setModelsOutput(result);
  } catch (error) {
    await setModelsOutput({ error: String(error) });
  }
});

document.querySelector<HTMLButtonElement>("#btn-models")?.addEventListener("click", async () => {
  try {
    const result = await invoke<ModelsResponse>("worker_list_models");
    await setModelsOutput(result);
  } catch (error) {
    await setModelsOutput({ error: String(error) });
  }
});

document.querySelector<HTMLButtonElement>("#btn-start")?.addEventListener("click", async () => {
  try {
    if (!apiKeyInput.value.trim()) {
      await setSessionOutput({ error: "Set your OPENAI_API_KEY first" });
      return;
    }
    const result = await invoke<{ session_id: string }>("worker_start_session", { profileId: "default" });
    currentSessionId = result.session_id;
    finalTranscript = "";
    await setTranscriptOutput("Session started. Listening...");
    await setSessionOutput({ status: "started", session_id: currentSessionId });
    startPolling();
  } catch (error) {
    await setSessionOutput({ error: String(error) });
  }
});

document.querySelector<HTMLButtonElement>("#btn-record")?.addEventListener("click", async () => {
  try {
    await startMicCapture();
  } catch (error) {
    await setSessionOutput(normalizeMicError(error));
  }
});

document.querySelector<HTMLButtonElement>("#btn-finalize")?.addEventListener("click", async () => {
  try {
    await stopMicAndFinalize();
  } catch (error) {
    await setSessionOutput({ error: String(error) });
  }
});

document.querySelector<HTMLButtonElement>("#btn-stop")?.addEventListener("click", async () => {
  if (!currentSessionId) {
    await setSessionOutput({ error: "No active session" });
    return;
  }
  if (mediaRecorder && isRecording) {
    mediaRecorder.stop();
  }
  try {
    const result = await invoke("worker_stop_session", { sessionId: currentSessionId });
    await setSessionOutput(result);
    currentSessionId = null;
    stopPolling();
  } catch (error) {
    await setSessionOutput({ error: String(error) });
  }
});

document.querySelector<HTMLButtonElement>("#btn-save-key")?.addEventListener("click", async () => {
  try {
    const openaiApiKey = apiKeyInput.value.trim();
    await invoke<AppSettings>("app_save_settings", { openaiApiKey });
    await setSettingsOutput({
      hasOpenAiApiKey: Boolean(openaiApiKey),
      status: openaiApiKey ? "saved" : "cleared",
    });
  } catch (error) {
    await setSettingsOutput({ error: String(error) });
  }
});

if (navigator.mediaDevices?.addEventListener) {
  navigator.mediaDevices.addEventListener("devicechange", () => {
    void loadMicDevices();
  });
}
void loadAppSettings();
void loadMicDevices();

