<script lang="ts">
  import { onMount } from "svelte";

  import CommandLog from "./lib/components/CommandLog.svelte";
  import DemoPanel from "./lib/components/DemoPanel.svelte";
  import Header from "./lib/components/Header.svelte";
  import Timeline from "./lib/components/Timeline.svelte";
  import {
    audioInfo,
    call,
    errorMessage,
    openFile,
    renderPath,
    setMasterGain,
    transportPlay,
    transportSeek,
    transportState,
    transportStop,
  } from "./lib/api";
  import type { Clip } from "./lib/timeline";

  type TrackResult = { track_id: string; path: string };
  type DeviceResult = { device_id: string; path: string };
  type RenderResult = {
    path: string;
    frames: number;
    duration_seconds: number;
    peak: number;
    rms: number;
  };

  const duration = 5;

  let trackId = $state("");
  let deviceId = $state("");
  let gain = $state(50);
  let busy = $state(false);
  let playing = $state(false);
  let audioAvailable = $state(true);
  let audioLabel = $state("AUDIO --");
  let status = $state("正在初始化…");
  let logLines = $state<string[]>([]);
  let playhead = $state(0);
  let clips = $state<Clip[]>([]);
  let lastSeekAt = 0;
  let polling = false;

  let trackLabel = $derived(
    trackId ? `TRACK ${trackId.toUpperCase()}` : "NO TRACK",
  );

  function log(line: string) {
    const stamp = new Date().toLocaleTimeString();
    logLines = [`[${stamp}] ${line}`, ...logLines].slice(0, 80);
  }

  async function run<T>(
    cmd: string,
    args: Record<string, unknown> = {},
  ): Promise<T> {
    log(`→ ${cmd} ${JSON.stringify(args)}`);
    return call<T>(cmd, args);
  }

  function gainPath() {
    return `/tracks/${trackId}/devices/${deviceId}/params/gain`;
  }

  async function refreshAudio() {
    try {
      const info = await audioInfo();
      audioAvailable = info.available;
      audioLabel = info.available ? `AUDIO ${info.sample_rate} HZ` : "NO AUDIO";
    } catch {
      audioAvailable = false;
      audioLabel = "AUDIO ?";
    }
  }

  async function setupDemo() {
    if (busy) return;
    busy = true;
    try {
      await transportStop();
      playing = false;
      await refreshAudio();
      status = "正在创建演示工程…";
      await run("project.create", { name: "Aurtrack Demo" });
      const track = await run<TrackResult>("track.create", {
        name: "Bass",
        source: "sine",
        frequency: 110,
      });
      const device = await run<DeviceResult>("device.add", {
        track_id: track.track_id,
        kind: "gain",
      });
      trackId = track.track_id;
      deviceId = device.device_id;
      await run("param.set", { path: gainPath(), value: gain / 100 });
      await setMasterGain(gain / 100);
      await transportSeek(0);
      clips = [{ id: "clip-1", name: "BASS / 110 HZ", start: 0, duration }];
      playhead = 0;
      status = audioAvailable
        ? "工程就绪。按 PLAY 试听，或拖动播放头定位。"
        : "工程就绪，但没有输出设备；播放头会走，听不到声音。";
    } catch (error) {
      status = `初始化失败：${errorMessage(error)}`;
    } finally {
      busy = false;
    }
  }

  async function play() {
    if (busy || playing) return;
    try {
      await setMasterGain(gain / 100);
      await transportPlay();
      playing = true;
      status = "PLAYING";
    } catch (error) {
      status = `播放失败：${errorMessage(error)}`;
    }
  }

  async function stop() {
    try {
      await transportStop();
      playing = false;
      status = "STOPPED";
    } catch (error) {
      status = `停止失败：${errorMessage(error)}`;
    }
  }

  function seek(seconds: number) {
    playhead = seconds;
    lastSeekAt = performance.now();
    void transportSeek(seconds).catch(() => {});
  }

  async function render() {
    if (busy || !trackId || !deviceId) return;
    busy = true;
    try {
      await transportStop();
      playing = false;
      status = "正在渲染 WAV…";
      await run("param.set", { path: gainPath(), value: gain / 100 });
      const path = await renderPath();
      const data = await run<RenderResult>("render.segment", {
        duration_seconds: duration,
        path,
      });
      await openFile(data.path);
      playhead = duration;
      status = `已渲染：peak ${data.peak.toFixed(3)} · rms ${data.rms.toFixed(3)} · ${data.frames} 帧`;
    } catch (error) {
      status = `渲染失败：${errorMessage(error)}`;
    } finally {
      busy = false;
    }
  }

  function onGainChange(value: number) {
    gain = value;
    void setMasterGain(value / 100).catch(() => {});
  }

  async function syncTransport() {
    if (polling) return;
    polling = true;
    try {
      const snapshot = await transportState();
      playing = snapshot.playing;
      if (snapshot.playing && performance.now() - lastSeekAt > 200) {
        playhead = snapshot.position;
      }
    } catch {
      // 轮询失败时保持现状，不打断交互。
    } finally {
      polling = false;
    }
  }

  onMount(() => {
    void setupDemo();
    const timer = window.setInterval(() => {
      void syncTransport();
    }, 33);
    return () => window.clearInterval(timer);
  });
</script>

<main class="sheet">
  <Header {audioLabel} />
  <Timeline
    trackName={trackLabel}
    {duration}
    {playhead}
    {clips}
    onSeek={seek}
  />
  <div class="split">
    <DemoPanel
      {gain}
      {busy}
      {playing}
      {status}
      {trackLabel}
      onGainChange={onGainChange}
      onPlay={play}
      onStop={stop}
      onRender={render}
      onReset={setupDemo}
    />
    <CommandLog lines={logLines} />
  </div>
</main>
