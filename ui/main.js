const { invoke } = window.__TAURI__.core;

const elements = {
  gain: document.getElementById("gain"),
  gainValue: document.getElementById("gain-value"),
  status: document.getElementById("status"),
  track: document.getElementById("track"),
  render: document.getElementById("render"),
  reset: document.getElementById("reset"),
  log: document.getElementById("log"),
};

const state = {
  trackId: null,
  deviceId: null,
  busy: false,
};

function gainValue() {
  return Number(elements.gain.value) / 100;
}

function gainPath() {
  return `/tracks/${state.trackId}/devices/${state.deviceId}/params/gain`;
}

function errorMessage(error) {
  if (typeof error === "string") return error;
  if (error && typeof error.message === "string") return error.message;
  return JSON.stringify(error);
}

function appendLog(line) {
  const stamp = new Date().toLocaleTimeString();
  const next = `[${stamp}] ${line}\n${elements.log.textContent}`;
  elements.log.textContent = next.slice(0, 4000);
}

async function call(cmd, args = {}) {
  appendLog(`→ ${cmd} ${JSON.stringify(args)}`);
  const response = await invoke("execute", { request: { cmd, args } });
  if (!response || response.ok !== true) {
    throw new Error(response?.error?.message ?? "命令失败");
  }
  return response.data;
}

function setStatus(message) {
  elements.status.textContent = message;
}

function setBusy(busy) {
  state.busy = busy;
  elements.render.disabled = busy;
  elements.reset.disabled = busy;
}

async function setupDemo() {
  if (state.busy) return;
  setBusy(true);
  try {
    setStatus("正在创建演示工程…");
    await call("project.create", { name: "Aurtrack Demo" });
    const track = await call("track.create", {
      name: "Bass",
      source: "sine",
      frequency: 110,
    });
    const device = await call("device.add", {
      track_id: track.track_id,
      kind: "gain",
    });
    state.trackId = track.track_id;
    state.deviceId = device.device_id;
    await call("param.set", { path: gainPath(), value: gainValue() });
    elements.track.textContent = `${track.track_id} / ${device.device_id}`;
    setStatus("工程就绪。拖动增益，然后点「渲染并播放」。");
  } catch (error) {
    setStatus(`初始化失败：${errorMessage(error)}`);
  } finally {
    setBusy(false);
  }
}

async function renderAndPlay() {
  if (state.busy || !state.trackId || !state.deviceId) return;
  setBusy(true);
  try {
    setStatus("正在渲染…");
    await call("param.set", { path: gainPath(), value: gainValue() });
    const path = await invoke("render_path");
    const data = await call("render.segment", {
      duration_seconds: 5,
      path,
    });
    await invoke("open_file", { path: data.path });
    setStatus(
      `已渲染：peak ${data.peak.toFixed(3)} · rms ${data.rms.toFixed(
        3
      )} · ${data.frames} 帧`
    );
  } catch (error) {
    setStatus(`渲染失败：${errorMessage(error)}`);
  } finally {
    setBusy(false);
  }
}

elements.gain.addEventListener("input", () => {
  elements.gainValue.textContent = `${Math.round(gainValue() * 100)}%`;
});
elements.render.addEventListener("click", renderAndPlay);
elements.reset.addEventListener("click", setupDemo);

window.addEventListener("DOMContentLoaded", () => {
  elements.gainValue.textContent = `${Math.round(gainValue() * 100)}%`;
  setupDemo();
});
