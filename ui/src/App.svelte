<script lang="ts">
  import { onMount } from "svelte";

  import CommandLog from "./lib/components/CommandLog.svelte";
  import ClipEditor from "./lib/components/ClipEditor.svelte";
  import Header from "./lib/components/Header.svelte";
  import PianoRoll from "./lib/components/PianoRoll.svelte";
  import Timeline from "./lib/components/Timeline.svelte";
  import TrackInspector from "./lib/components/TrackInspector.svelte";
  import TrackList from "./lib/components/TrackList.svelte";
  import TransportPanel from "./lib/components/TransportPanel.svelte";
  import {
    audioInfo,
    call,
    errorMessage,
    openFile,
    pickProjectPath,
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
  import {
    errorText,
    formatClock,
    formatNumber,
    t,
    type MessageKey,
  } from "./lib/i18n.svelte";

  type TrackResult = { track_id: string };
  type RenderResult = {
    path: string;
    frames: number;
    peak: number;
    rms: number;
  };
  type Status = {
    key: MessageKey;
    params?: Record<string, string | number>;
  };

  const duration = 5;

  let project = $state<Project | null>(null);
  let selectedTrackId = $state("");
  let selectedClipId = $state("");
  let projectPath = $state("");
  let gain = $state(50);
  let busy = $state(false);
  let playing = $state(false);
  let audioState = $state<"ok" | "none" | "unknown">("unknown");
  let sampleRate = $state(48_000);
  let status = $state<Status>({ key: "status.connecting" });
  let statusText = $derived(t(status.key, status.params ?? {}));
  let logLines = $state<string[]>([]);
  let playhead = $state(0);
  let commandCount = $state(0);
  let lastSeekAt = 0;
  let polling = false;
  let projectPolling = false;

  let tracks = $derived(project?.tracks ?? []);
  let projectFile = $derived(fileName(projectPath));
  let selectedTrack = $derived(
    tracks.find((track) => track.id === selectedTrackId) ?? tracks[0] ?? null,
  );
  let selectedClip = $derived(
    selectedTrack?.clips.find((clip) => clip.id === selectedClipId) ??
      selectedTrack?.clips[0] ??
      null,
  );

  $effect(() => {
    const clips = selectedTrack?.clips ?? [];
    if (clips.length === 0) {
      if (selectedClipId !== "") selectedClipId = "";
    } else if (!clips.some((clip) => clip.id === selectedClipId)) {
      selectedClipId = clips[0]?.id ?? "";
    }
  });
  let timelineTracks = $derived(
    tracks.map((track) => {
      const noteCount = track.clips.reduce(
        (sum, clip) => sum + clip.notes.length,
        0,
      );
      return {
        id: track.id,
        name: track.name,
        frequency:
          track.source.kind === "sine" ? track.source.frequency : null,
        meta:
          track.clips.length > 0
            ? t("timeline.clipMeta", {
                clips: track.clips.length,
                notes: noteCount,
              })
            : track.source.kind === "sine"
              ? t("timeline.trackMeta", {
                  frequency: formatNumber(track.source.frequency, 2),
                  devices: track.devices.length,
                })
              : t("timeline.silenceMeta", {
                  devices: track.devices.length,
                }),
        clips: track.clips.map((clip) => ({
          id: clip.id,
          name: clip.name,
          start: clip.start,
          duration: clip.duration,
          notes: clip.notes.map((note) => ({
            id: note.id,
            pitch: note.pitch,
            start: note.start,
            duration: note.duration,
          })),
        })),
      };
    }),
  );

  function setStatus(
    key: MessageKey,
    params?: Record<string, string | number>,
  ) {
    status = { key, params };
  }

  function fileName(path: string): string {
    const parts = path.split(/[\\/]/);
    return parts[parts.length - 1] ?? path;
  }

  function errorMessageFor(error: unknown): string {
    if (
      typeof error === "object" &&
      error !== null &&
      "code" in error &&
      "message" in error
    ) {
      const detail = error as { code: string; message: string };
      return errorText(detail.code, detail.message);
    }
    return errorMessage(error);
  }

  function fail(key: MessageKey, error: unknown) {
    setStatus(key, { message: errorMessageFor(error) });
  }

  function log(line: string) {
    const stamp = formatClock(new Date());
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
      audioState = info.available ? "ok" : "none";
      sampleRate = info.sample_rate;
    } catch {
      audioState = "unknown";
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
        await call("project.create", { name: t("project.defaultName") });
        const created = await projectState();
        project = created.project;
      }
      selectedTrackId = project?.tracks[0]?.id ?? "";
      const description = await call<{ commands: { id: string }[] }>("describe");
      commandCount = description.commands.length;
      setStatus("status.ready");
    } catch (error) {
      fail("status.initFailed", error);
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
      setStatus("status.trackCreated", { id: result.track_id });
    } catch (error) {
      fail("status.trackCreateFailed", error);
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
      setStatus("status.trackRemoved", { id: trackId });
    } catch (error) {
      fail("status.trackRemoveFailed", error);
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
      setStatus("status.gainAdded");
    } catch (error) {
      fail("status.gainAddFailed", error);
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
      setStatus("status.deviceRemoved");
    } catch (error) {
      fail("status.deviceRemoveFailed", error);
    } finally {
      busy = false;
    }
  }

  async function addClip(
    trackId: string,
    input: { name: string; start: number; duration: number },
  ) {
    if (busy) return;
    busy = true;
    try {
      const result = await run<{ clip_id: string }>("clip.create", {
        track_id: trackId,
        ...input,
      });
      await refreshProject();
      await reloadIfPlaying();
      selectedClipId = result.clip_id;
      setStatus("status.clipCreated", { id: result.clip_id });
    } catch (error) {
      fail("status.clipCreateFailed", error);
    } finally {
      busy = false;
    }
  }

  async function deleteClip(path: string) {
    if (busy) return;
    busy = true;
    try {
      await run("clip.remove", { path });
      await refreshProject();
      await reloadIfPlaying();
      const id = path.split("/").pop() ?? path;
      setStatus("status.clipRemoved", { id });
    } catch (error) {
      fail("status.clipRemoveFailed", error);
    } finally {
      busy = false;
    }
  }

  async function addNote(
    path: string,
    input: {
      pitch: number;
      start: number;
      duration: number;
      velocity: number;
    },
  ) {
    if (busy) return;
    busy = true;
    try {
      const result = await run<{ note_id: string }>("note.add", {
        path,
        ...input,
      });
      await refreshProject();
      await reloadIfPlaying();
      setStatus("status.noteAdded", { id: result.note_id });
    } catch (error) {
      fail("status.noteAddFailed", error);
    } finally {
      busy = false;
    }
  }

  async function deleteNote(path: string) {
    if (busy) return;
    busy = true;
    try {
      await run("note.remove", { path });
      await refreshProject();
      await reloadIfPlaying();
      const id = path.split("/").pop() ?? path;
      setStatus("status.noteRemoved", { id });
    } catch (error) {
      fail("status.noteRemoveFailed", error);
    } finally {
      busy = false;
    }
  }

  function selectClip(trackId: string, clipId: string) {
    selectedTrackId = trackId;
    selectedClipId = clipId;
  }

  async function updateClip(
    path: string,
    patch: { start?: number; duration?: number },
  ) {
    if (busy) return;
    busy = true;
    try {
      await run("clip.update", { path, ...patch });
      await refreshProject();
      await reloadIfPlaying();
      setStatus("status.clipUpdated", {
        id: path.split("/").pop() ?? path,
      });
    } catch (error) {
      fail("status.clipUpdateFailed", error);
    } finally {
      busy = false;
    }
  }

  async function updateNote(
    path: string,
    patch: {
      pitch?: number;
      start?: number;
      duration?: number;
      velocity?: number;
    },
  ) {
    if (busy) return;
    busy = true;
    try {
      await run("note.update", { path, ...patch });
      await refreshProject();
      await reloadIfPlaying();
      setStatus("status.noteUpdated", {
        id: path.split("/").pop() ?? path,
      });
    } catch (error) {
      fail("status.noteUpdateFailed", error);
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
      fail("status.gainSetFailed", error);
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
      setStatus("status.undone");
    } catch (error) {
      fail("status.undoFailed", error);
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
      setStatus("status.redone");
    } catch (error) {
      fail("status.redoFailed", error);
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
      setStatus("status.playing");
    } catch (error) {
      fail("status.playFailed", error);
    }
  }

  async function stop() {
    try {
      await transportStop();
      playing = false;
      setStatus("status.stopped");
    } catch (error) {
      fail("status.stopFailed", error);
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
      setStatus("status.rendering");
      const path = await renderPath();
      const data = await run<RenderResult>("render.segment", {
        duration_seconds: duration,
        path,
      });
      await openFile(data.path);
      playhead = duration;
      setStatus("status.rendered", {
        peak: formatNumber(data.peak, 3),
        rms: formatNumber(data.rms, 3),
        frames: formatNumber(data.frames),
      });
    } catch (error) {
      fail("status.renderFailed", error);
    } finally {
      busy = false;
    }
  }

  async function saveProject() {
    if (busy) return;
    let path: string | null;
    try {
      path = await pickProjectPath("save");
    } catch (error) {
      fail("status.saveFailed", error);
      return;
    }
    if (!path) return;

    busy = true;
    try {
      await run("project.save", { path });
      projectPath = path;
      setStatus("status.saved", { file: fileName(path) });
    } catch (error) {
      fail("status.saveFailed", error);
    } finally {
      busy = false;
    }
  }

  async function openProject() {
    if (busy) return;
    let path: string | null;
    try {
      path = await pickProjectPath("open");
    } catch (error) {
      fail("status.openFailed", error);
      return;
    }
    if (!path) return;

    busy = true;
    try {
      await run("project.open", { path });
      await refreshProject();
      await reloadIfPlaying();
      projectPath = path;
      setStatus("status.opened", { file: fileName(path) });
    } catch (error) {
      fail("status.openFailed", error);
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
  <Header {audioState} {sampleRate} />
  <Timeline
    tracks={timelineTracks}
    {duration}
    {playhead}
    onSeek={seek}
    onSelectClip={selectClip}
    onUpdateClip={updateClip}
    onUpdateNote={updateNote}
  />
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
      <ClipEditor
        track={selectedTrack}
        {busy}
        {selectedClipId}
        onSelectClip={(id: string) => (selectedClipId = id)}
        onAddClip={addClip}
        onRemoveClip={deleteClip}
        onAddNote={addNote}
        onRemoveNote={deleteNote}
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
        status={statusText}
        fileName={projectFile}
        {commandCount}
        onPlay={play}
        onStop={stop}
        onGainChange={onGainChange}
        onRender={render}
        onSave={saveProject}
        onOpen={openProject}
        onUndo={undo}
        onRedo={redo}
      />
    </div>
  </div>
  <PianoRoll
    trackId={selectedTrack?.id ?? null}
    clip={selectedClip}
    {busy}
    onAddNote={addNote}
    onUpdateNote={updateNote}
    onRemoveNote={deleteNote}
  />
  <CommandLog lines={logLines} />
</main>
