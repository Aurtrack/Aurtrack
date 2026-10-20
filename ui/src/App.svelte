<script lang="ts">
  import { onMount } from "svelte";

  import CommandLog from "./lib/components/CommandLog.svelte";
  import Header from "./lib/components/Header.svelte";
  import Timeline from "./lib/components/Timeline.svelte";
  import TrackInspector from "./lib/components/TrackInspector.svelte";
  import TrackList from "./lib/components/TrackList.svelte";
  import TransportPanel from "./lib/components/TransportPanel.svelte";
  import {
    audioInfo,
    call,
    errorMessage,
    openFile,
    projectState,
    renderPath,
    setMasterGain,
    transportPlay,
    transportSeek,
    transportState,
    transportStop,
    type Project,
    type ProjectState,
  } from "./lib/api";

  type TrackResult = { track_id: string };
  type RenderResult = {
    path: string;
    frames: number;
    peak: number;
    rms: number;
  };

  const duration = 5;

  let project = $state<Project | null>(null);
  let selectedTrackId = $state("");
  let gain = $state(50);
  let busy = $state(false);
  let playing = $state(false);
  let audioAvailable = $state(true);
  let audioLabel = $state("AUDIO --");
  let status = $state("正在连接会话…");
  let logLines = $state<string[]>([]);
  let playhead = $state(0);
  let commandCount = $state(0);
  let lastSeekAt = 0;
  let polling = false;
  let projectPolling = false;

  let tracks = $derived(project?.tracks ?? []);
  let selectedTrack = $derived(
    tracks.find((track) => track.id === selectedTrackId) ?? tracks[0] ?? null,
  );
  let timelineTracks = $derived(
    tracks.map((track) => ({
      id: track.id,
      name: track.name,
      frequency: track.source.kind === "sine" ? track.source.frequency : null,
      deviceCount: track.devices.length,
    })),
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

  async function refreshProject() {
    if (projectPolling) return;
    projectPolling = true;
    try {
      const state = await projectState();
      project = state.project;
      const list = state.project?.tracks ?? [];
      if (!list.some((track) => track.id === selectedTrackId)) {
        selectedTrackId = list[0]?.id ?? "";
      }
    } catch {
      // 轮询失败时保持现状，不打断交互。
    } finally {
      projectPolling = false;
    }
  }

  async function bootstrap() {
    await refreshAudio();
    try {
      const state = await projectState();
      project = state.project;
      if (!state.project) {
        await call("project.create", { name: "Aurtrack Project" });
        const created = await projectState();
        project = created.project;
      }
      selectedTrackId = project?.tracks[0]?.id ?? "";
      const description = await call<{ commands: { id: string }[] }>("describe");
      commandCount = description.commands.length;
      status = "会话就绪。添加轨道，或按 PLAY。";
    } catch (error) {
      status = `初始化失败：${errorMessage(error)}`;
    }
  }

  async function addTrack(input: {
    name: string;
    source: "sine" | "silence";
    frequency: number;
  }) {
    if (busy) return;
    busy = true;
    try {
      const result = await run<TrackResult>("track.create", {
        name: input.name,
        source: input.source,
        frequency: input.frequency,
      });
      await refreshProject();
      selectedTrackId = result.track_id;
      await reloadIfPlaying();
      status = `已创建 ${result.track_id}`;
    } catch (error) {
      status = `建轨失败：${errorMessage(error)}`;
    } finally {
      busy = false;
    }
  }

  async function deleteTrack(trackId: string) {
    if (busy) return;
    busy = true;
    try {
      await run("track.remove", { track_id: trackId });
      await refreshProject();
      await reloadIfPlaying();
      status = `已删除 ${trackId}`;
    } catch (error) {
      status = `删除轨道失败：${errorMessage(error)}`;
    } finally {
      busy = false;
    }
  }

  async function addGain(trackId: string) {
    if (busy) return;
    busy = true;
    try {
      await run("device.add", { track_id: trackId, kind: "gain" });
      await refreshProject();
      await reloadIfPlaying();
      status = "已添加 gain";
    } catch (error) {
      status = `添加设备失败：${errorMessage(error)}`;
    } finally {
      busy = false;
    }
  }

  async function deleteDevice(path: string) {
    if (busy) return;
    busy = true;
    try {
      await run("device.remove", { path });
      await refreshProject();
      await reloadIfPlaying();
      status = "已删除设备";
    } catch (error) {
      status = `删除设备失败：${errorMessage(error)}`;
    } finally {
      busy = false;
    }
  }

  async function changeDeviceGain(
    trackId: string,
    deviceId: string,
    value: number,
  ) {
    try {
      await run("param.set", {
        path: `/tracks/${trackId}/devices/${deviceId}/params/gain`,
        value,
      });
      if (playing) {
        await transportPlay();
      }
      await refreshProject();
    } catch (error) {
      status = `设置增益失败：${errorMessage(error)}`;
    }
  }

  async function reloadIfPlaying() {
    if (!playing) return;
    try {
      await transportPlay();
    } catch {
      // 重建失败时保留下一次 PLAY 的机会，不覆盖当前状态。
    }
  }

  async function undo() {
    if (busy) return;
    busy = true;
    try {
      await run("undo", { steps: 1 });
      await refreshProject();
      await reloadIfPlaying();
      status = "已撤销";
    } catch (error) {
      status = `撤销失败：${errorMessage(error)}`;
    } finally {
      busy = false;
    }
  }

  async function redo() {
    if (busy) return;
    busy = true;
    try {
      await run("redo", { steps: 1 });
      await refreshProject();
      await reloadIfPlaying();
      status = "已重做";
    } catch (error) {
      status = `重做失败：${errorMessage(error)}`;
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
    if (busy) return;
    busy = true;
    try {
      await transportStop();
      playing = false;
      status = "正在渲染 WAV…";
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
    void bootstrap();
    const transportTimer = window.setInterval(() => {
      void syncTransport();
    }, 33);
    const projectTimer = window.setInterval(() => {
      void refreshProject();
    }, 800);
    return () => {
      window.clearInterval(transportTimer);
      window.clearInterval(projectTimer);
    };
  });
</script>

<main class="sheet">
  <Header {audioLabel} />
  <Timeline tracks={timelineTracks} {duration} {playhead} onSeek={seek} />
  <div class="split">
    <div class="column">
      <TrackList
        {tracks}
        selectedId={selectedTrack?.id ?? ""}
        {busy}
        onSelect={(id: string) => (selectedTrackId = id)}
        onAdd={addTrack}
        onRemove={deleteTrack}
      />
    </div>
    <div class="column">
      <TrackInspector
        track={selectedTrack}
        {busy}
        onAddGain={addGain}
        onRemoveDevice={deleteDevice}
        onGainChange={changeDeviceGain}
      />
      <TransportPanel
        {playing}
        {busy}
        {gain}
        {status}
        {commandCount}
        onPlay={play}
        onStop={stop}
        onGainChange={onGainChange}
        onRender={render}
        onUndo={undo}
        onRedo={redo}
      />
    </div>
  </div>
  <CommandLog lines={logLines} />
</main>
