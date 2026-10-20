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
} from "./api";
import {
  errorText,
  formatClock,
  formatNumber,
  t,
  type MessageKey,
} from "./i18n.svelte";

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

function fileName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] ?? path;
}

export class SessionStore {
  readonly duration = 5;

  private project = $state<Project | null>(null);
  selectedTrackId = $state("");
  selectedClipId = $state("");
  private projectPath = $state("");
  gain = $state(50);
  busy = $state(false);
  playing = $state(false);
  audioState = $state<"ok" | "none" | "unknown">("unknown");
  sampleRate = $state(48_000);
  private status = $state<Status>({ key: "status.connecting" });
  logLines = $state<string[]>([]);
  playhead = $state(0);
  commandCount = $state(0);
  private lastSeekAt = 0;
  private polling = false;
  private projectPolling = false;
  private transportTimer: number | null = null;
  private projectTimer: number | null = null;
  private started = false;

  tracks = $derived(this.project?.tracks ?? []);
  projectFile = $derived(fileName(this.projectPath));
  selectedTrack = $derived(
    this.tracks.find((track) => track.id === this.selectedTrackId) ??
      this.tracks[0] ??
      null,
  );
  selectedClip = $derived(
    this.selectedTrack?.clips.find((clip) => clip.id === this.selectedClipId) ??
      this.selectedTrack?.clips[0] ??
      null,
  );
  statusText = $derived(t(this.status.key, this.status.params ?? {}));
  timelineTracks = $derived(this.buildTimelineTracks());

  start() {
    if (this.started) return;
    this.started = true;
    void this.bootstrap();
    this.transportTimer = window.setInterval(() => {
      void this.syncTransport();
    }, 33);
    this.projectTimer = window.setInterval(() => {
      void this.refreshProject();
    }, 800);
  }

  dispose() {
    if (!this.started) return;
    this.started = false;
    if (this.transportTimer !== null) {
      window.clearInterval(this.transportTimer);
      this.transportTimer = null;
    }
    if (this.projectTimer !== null) {
      window.clearInterval(this.projectTimer);
      this.projectTimer = null;
    }
  }

  selectTrack(trackId: string) {
    this.selectedTrackId = trackId;
    const track = this.project?.tracks.find((item) => item.id === trackId);
    this.selectedClipId = track?.clips[0]?.id ?? "";
  }

  selectClip(trackId: string, clipId: string) {
    this.selectedTrackId = trackId;
    this.selectedClipId = clipId;
  }

  selectClipId(clipId: string) {
    const track = this.selectedTrack;
    if (track) this.selectedClipId = clipId;
  }

  async addTrack(input: {
    name: string;
    source: "sine" | "silence";
    frequency: number;
  }) {
    if (this.busy) return;
    this.busy = true;
    try {
      const result = await this.run<TrackResult>("track.create", {
        name: input.name,
        source: input.source,
        frequency: input.frequency,
      });
      await this.refreshProject();
      this.selectedTrackId = result.track_id;
      this.selectedClipId = "";
      await this.reloadIfPlaying();
      this.setStatus("status.trackCreated", { id: result.track_id });
    } catch (error) {
      this.fail("status.trackCreateFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async deleteTrack(trackId: string) {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.run("track.remove", { track_id: trackId });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.setStatus("status.trackRemoved", { id: trackId });
    } catch (error) {
      this.fail("status.trackRemoveFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async addGain(trackId: string) {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.run("device.add", { track_id: trackId, kind: "gain" });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.setStatus("status.gainAdded");
    } catch (error) {
      this.fail("status.gainAddFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async deleteDevice(path: string) {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.run("device.remove", { path });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.setStatus("status.deviceRemoved");
    } catch (error) {
      this.fail("status.deviceRemoveFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async changeDeviceGain(
    trackId: string,
    deviceId: string,
    value: number,
  ) {
    try {
      await this.run("param.set", {
        path: `/tracks/${trackId}/devices/${deviceId}/params/gain`,
        value,
      });
      await this.reloadIfPlaying();
      await this.refreshProject();
    } catch (error) {
      this.fail("status.gainSetFailed", error);
    }
  }

  async addClip(
    trackId: string,
    input: { name: string; start: number; duration: number },
  ) {
    if (this.busy) return;
    this.busy = true;
    try {
      const result = await this.run<{ clip_id: string }>("clip.create", {
        track_id: trackId,
        ...input,
      });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.selectedClipId = result.clip_id;
      this.setStatus("status.clipCreated", { id: result.clip_id });
    } catch (error) {
      this.fail("status.clipCreateFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async deleteClip(path: string) {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.run("clip.remove", { path });
      await this.refreshProject();
      await this.reloadIfPlaying();
      const id = path.split("/").pop() ?? path;
      this.setStatus("status.clipRemoved", { id });
    } catch (error) {
      this.fail("status.clipRemoveFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async addNote(
    path: string,
    input: {
      pitch: number;
      start: number;
      duration: number;
      velocity: number;
    },
  ) {
    if (this.busy) return;
    this.busy = true;
    try {
      const result = await this.run<{ note_id: string }>("note.add", {
        path,
        ...input,
      });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.setStatus("status.noteAdded", { id: result.note_id });
    } catch (error) {
      this.fail("status.noteAddFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async deleteNote(path: string) {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.run("note.remove", { path });
      await this.refreshProject();
      await this.reloadIfPlaying();
      const id = path.split("/").pop() ?? path;
      this.setStatus("status.noteRemoved", { id });
    } catch (error) {
      this.fail("status.noteRemoveFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async updateClip(
    path: string,
    patch: { start?: number; duration?: number },
  ) {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.run("clip.update", { path, ...patch });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.setStatus("status.clipUpdated", {
        id: path.split("/").pop() ?? path,
      });
    } catch (error) {
      this.fail("status.clipUpdateFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async updateNote(
    path: string,
    patch: {
      pitch?: number;
      start?: number;
      duration?: number;
      velocity?: number;
    },
  ) {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.run("note.update", { path, ...patch });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.setStatus("status.noteUpdated", {
        id: path.split("/").pop() ?? path,
      });
    } catch (error) {
      this.fail("status.noteUpdateFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async undo() {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.run("undo", { steps: 1 });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.setStatus("status.undone");
    } catch (error) {
      this.fail("status.undoFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async redo() {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.run("redo", { steps: 1 });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.setStatus("status.redone");
    } catch (error) {
      this.fail("status.redoFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async play() {
    if (this.busy || this.playing) return;
    try {
      await setMasterGain(this.gain / 100);
      await transportPlay();
      this.playing = true;
      this.setStatus("status.playing");
    } catch (error) {
      this.fail("status.playFailed", error);
    }
  }

  async stop() {
    try {
      await transportStop();
      this.playing = false;
      this.setStatus("status.stopped");
    } catch (error) {
      this.fail("status.stopFailed", error);
    }
  }

  seek(seconds: number) {
    this.playhead = seconds;
    this.lastSeekAt = performance.now();
    void transportSeek(seconds).catch(() => {});
  }

  async render() {
    if (this.busy) return;
    this.busy = true;
    try {
      await transportStop();
      this.playing = false;
      this.setStatus("status.rendering");
      const path = await renderPath();
      const data = await this.run<RenderResult>("render.segment", {
        duration_seconds: this.duration,
        path,
      });
      await openFile(data.path);
      this.playhead = this.duration;
      this.setStatus("status.rendered", {
        peak: formatNumber(data.peak, 3),
        rms: formatNumber(data.rms, 3),
        frames: formatNumber(data.frames),
      });
    } catch (error) {
      this.fail("status.renderFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async saveProject() {
    if (this.busy) return;
    let path: string | null;
    try {
      path = await pickProjectPath("save");
    } catch (error) {
      this.fail("status.saveFailed", error);
      return;
    }
    if (!path) return;

    this.busy = true;
    try {
      await this.run("project.save", { path });
      this.projectPath = path;
      this.setStatus("status.saved", { file: fileName(path) });
    } catch (error) {
      this.fail("status.saveFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async openProject() {
    if (this.busy) return;
    let path: string | null;
    try {
      path = await pickProjectPath("open");
    } catch (error) {
      this.fail("status.openFailed", error);
      return;
    }
    if (!path) return;

    this.busy = true;
    try {
      await this.run("project.open", { path });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.projectPath = path;
      this.setStatus("status.opened", { file: fileName(path) });
    } catch (error) {
      this.fail("status.openFailed", error);
    } finally {
      this.busy = false;
    }
  }

  onGainChange(value: number) {
    this.gain = value;
    void setMasterGain(value / 100).catch(() => {});
  }

  private buildTimelineTracks() {
    return this.tracks.map((track) => {
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
    });
  }

  private setStatus(
    key: MessageKey,
    params?: Record<string, string | number>,
  ) {
    this.status = { key, params };
  }

  private errorMessageFor(error: unknown): string {
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

  private fail(key: MessageKey, error: unknown) {
    this.setStatus(key, { message: this.errorMessageFor(error) });
  }

  private log(line: string) {
    const stamp = formatClock(new Date());
    this.logLines = [`[${stamp}] ${line}`, ...this.logLines].slice(0, 80);
  }

  private async run<T>(
    cmd: string,
    args: Record<string, unknown> = {},
  ): Promise<T> {
    this.log(`→ ${cmd} ${JSON.stringify(args)}`);
    return call<T>(cmd, args);
  }

  private async refreshAudio() {
    try {
      const info = await audioInfo();
      this.audioState = info.available ? "ok" : "none";
      this.sampleRate = info.sample_rate;
    } catch {
      this.audioState = "unknown";
    }
  }

  private async refreshProject() {
    if (this.projectPolling) return;
    this.projectPolling = true;
    try {
      const state: ProjectState = await projectState();
      this.project = state.project;
      this.ensureSelection();
    } catch {
      // 轮询失败时保持现状，不打断交互。
    } finally {
      this.projectPolling = false;
    }
  }

  private ensureSelection() {
    const list = this.project?.tracks ?? [];
    const track =
      list.find((item) => item.id === this.selectedTrackId) ??
      list[0] ??
      null;
    const trackId = track?.id ?? "";
    if (this.selectedTrackId !== trackId) this.selectedTrackId = trackId;
    const clips = track?.clips ?? [];
    if (!clips.some((clip) => clip.id === this.selectedClipId)) {
      this.selectedClipId = clips[0]?.id ?? "";
    }
  }

  private async bootstrap() {
    await this.refreshAudio();
    try {
      const state: ProjectState = await projectState();
      this.project = state.project;
      if (!state.project) {
        await call("project.create", { name: t("project.defaultName") });
        const created: ProjectState = await projectState();
        this.project = created.project;
      }
      this.ensureSelection();
      const description = await call<{ commands: { id: string }[] }>(
        "describe",
      );
      this.commandCount = description.commands.length;
      this.setStatus("status.ready");
    } catch (error) {
      this.fail("status.initFailed", error);
    }
  }

  private async reloadIfPlaying() {
    if (!this.playing) return;
    try {
      await transportPlay();
    } catch {
      // 重建失败时保留下一次 PLAY 的机会，不覆盖当前状态。
    }
  }

  private async syncTransport() {
    if (this.polling) return;
    this.polling = true;
    try {
      const snapshot = await transportState();
      this.playing = snapshot.playing;
      if (
        snapshot.playing &&
        performance.now() - this.lastSeekAt > 200
      ) {
        this.playhead = snapshot.position;
      }
    } catch {
      // 轮询失败时保持现状，不打断交互。
    } finally {
      this.polling = false;
    }
  }
}

export const session = new SessionStore();
