import {
  audioInfo,
  call,
  destroyWindow,
  errorMessage,
  onCloseRequested,
  openFile,
  pickProjectPath,
  projectState,
  recoveryExists,
  recoveryPath,
  removeRecoveryFile,
  renderPath,
  requestWindowClose,
  setMasterGain,
  transportPlay,
  transportSeek,
  transportState,
  transportStop,
  type NodeGroup,
  type NodeTypeInfo,
  type Note,
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

export type RecentProject = {
  path: string;
  name: string;
  savedAt: number;
};

export type NoteSelectMode = "replace" | "toggle" | "range";

type ClipboardNote = {
  pitch: number;
  start: number;
  duration: number;
  velocity: number;
};

type Clipboard =
  | { kind: "notes"; notes: ClipboardNote[] }
  | {
      kind: "clip";
      name: string;
      duration: number;
      notes: ClipboardNote[];
    };

const MUTATING_COMMANDS = new Set([
  "track.create",
  "track.remove",
  "group.voice",
  "device.add",
  "device.remove",
  "param.set",
  "node.flags",
  "clip.create",
  "clip.update",
  "clip.remove",
  "clip.duplicate",
  "clip.move",
  "note.add",
  "note.update",
  "note.remove",
  "note.remove_many",
  "note.duplicate",
  "note.quantize",
  "note.quantize_many",
  "note.paste",
  "clip.paste",
  "undo",
  "redo",
  "project.open",
]);

const RECENT_KEY = "aurtrack.recent.v1";
const MAX_RECENT = 8;
const RECOVERY_DELAY_MS = 1200;

function fileName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] ?? path;
}

function isRecentProject(value: unknown): value is RecentProject {
  if (typeof value !== "object" || value === null) return false;
  const raw = value as Record<string, unknown>;
  return (
    typeof raw.path === "string" &&
    typeof raw.name === "string" &&
    typeof raw.savedAt === "number"
  );
}

function loadRecentProjects(): RecentProject[] {
  try {
    const raw = localStorage.getItem(RECENT_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(isRecentProject).slice(0, MAX_RECENT);
  } catch {
    return [];
  }
}

export class SessionStore {
  private project = $state<Project | null>(null);
  private nodeTypesState = $state<NodeTypeInfo[]>([]);
  private audioNodeTypesState = $state<NodeTypeInfo[]>([]);
  selectedTrackId = $state("");
  selectedClipId = $state("");
  selectedNoteIds = $state<string[]>([]);
  private selectionAnchorNoteId = $state("");
  private projectPath = $state("");
  gain = $state(50);
  busy = $state(false);
  playing = $state(false);
  metronome = $state(false);
  audioState = $state<"ok" | "none" | "unknown">("unknown");
  sampleRate = $state(48_000);
  private status = $state<Status>({ key: "status.connecting" });
  logLines = $state<string[]>([]);
  playhead = $state(0);
  commandCount = $state(0);
  dirty = $state(false);
  recoveryAvailable = $state(false);
  closeRequested = $state(false);
  pluginManagerOpen = $state(false);
  pendingOpenPath = $state<string | null>(null);
  clipboard = $state<Clipboard | null>(null);
  recentProjects = $state<RecentProject[]>(loadRecentProjects());
  private lastSeekAt = 0;
  private lastSeekSentAt = 0;
  private seekTimer: number | null = null;
  private pendingSeekSeconds: number | null = null;
  private lastTransportPosition = -1;
  private lastTransportProgressAt = 0;
  private audioRecoveryAttempts = 0;
  private recoveringAudio = false;
  private lastProjectSnapshot = "";
  private polling = false;
  private projectPolling = false;
  private transportTimer: number | null = null;
  private projectTimer: number | null = null;
  private recoveryTimer: number | null = null;
  private recoveryPath = "";
  private recoveryToken = 0;
  private recovering = false;
  private started = false;
  private closeGuardRegistered = false;
  private unlistenClose: (() => void) | null = null;

  tracks = $derived(this.project?.tracks ?? []);
  duration = $derived(this.project?.length_seconds ?? 5);
  tempo = $derived(this.project?.tempo_bpm ?? 120);
  timeSignature = $derived(
    this.project?.time_signature ?? { numerator: 4, denominator: 4 },
  );
  barSeconds = $derived(this.project?.bar_seconds ?? 2);
  secondsPerBeat = $derived(60 / Math.max(1, this.tempo));
  lengthBars = $derived(this.project?.length_bars ?? 4);
  loopStart = $derived(this.project?.loop_start ?? 0);
  loopEnd = $derived(this.project?.loop_end ?? this.duration);
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
  selectedNotes = $derived(
    this.selectedClip
      ? this.selectedClip.notes.filter((note) =>
          this.selectedNoteIds.includes(note.id),
        )
      : [],
  );
  selectedNoteId = $derived(this.selectedNoteIds.at(-1) ?? "");
  selectedNote = $derived(
    this.selectedNotes.find((note) => note.id === this.selectedNoteId) ?? null,
  );
  selectedClipPath = $derived(
    this.selectedTrack && this.selectedClip
      ? `/tracks/${this.selectedTrack.id}/clips/${this.selectedClip.id}`
      : null,
  );
  selectedNotePaths = $derived(
    this.selectedTrack && this.selectedClip
      ? this.selectedNotes.map(
          (note) =>
            `/tracks/${this.selectedTrack?.id}/clips/${this.selectedClip?.id}/notes/${note.id}`,
        )
      : [],
  );
  selectedNotePath = $derived(
    this.selectedNotePaths.length === 1 ? this.selectedNotePaths[0] : null,
  );
  statusText = $derived(t(this.status.key, this.status.params ?? {}));
  timelineTracks = $derived(this.buildTimelineTracks());

  start() {
    if (this.started) return;
    this.started = true;
    void this.bootstrap().then(() => {
      void this.checkRecovery();
    });
    void this.initCloseGuard();
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
    if (this.recoveryTimer !== null) {
      window.clearTimeout(this.recoveryTimer);
      this.recoveryTimer = null;
    }
    this.cancelSeek();
    if (this.unlistenClose) {
      this.unlistenClose();
      this.unlistenClose = null;
      this.closeGuardRegistered = false;
    }
    this.closeRequested = false;
  }

  selectTrack(trackId: string) {
    this.selectedTrackId = trackId;
    const track = this.project?.tracks.find((item) => item.id === trackId);
    this.selectedClipId = track?.clips[0]?.id ?? "";
    this.clearNoteSelection();
  }

  selectClip(trackId: string, clipId: string) {
    this.selectedTrackId = trackId;
    this.selectedClipId = clipId;
    this.clearNoteSelection();
  }

  selectClipId(clipId: string) {
    const track = this.selectedTrack;
    if (track) {
      this.selectedClipId = clipId;
      this.clearNoteSelection();
    }
  }

  selectNote(noteId: string, mode: NoteSelectMode = "replace") {
    const clip = this.selectedClip;
    const target = clip?.notes.find((note) => note.id === noteId);
    if (!clip || !target) return;

    if (mode === "toggle") {
      if (this.selectedNoteIds.includes(noteId)) {
        this.setNoteSelection(
          this.selectedNoteIds.filter((id) => id !== noteId),
        );
        if (this.selectionAnchorNoteId === noteId) {
          this.selectionAnchorNoteId = this.selectedNoteIds.at(-1) ?? "";
        }
        return;
      }
      this.setNoteSelection([...this.selectedNoteIds, noteId]);
      this.selectionAnchorNoteId = noteId;
      return;
    }

    if (mode === "range") {
      const anchor =
        clip.notes.find((note) => note.id === this.selectionAnchorNoteId) ??
        target;
      const low = Math.min(anchor.start, target.start);
      const high = Math.max(anchor.start, target.start);
      const ids = clip.notes
        .filter((note) => note.start >= low - 1e-9 && note.start <= high + 1e-9)
        .map((note) => note.id)
        .filter((id) => id !== noteId);
      this.setNoteSelection([...ids, noteId]);
      this.selectionAnchorNoteId = anchor.id;
      return;
    }

    this.setNoteSelection([noteId]);
    this.selectionAnchorNoteId = noteId;
  }

  selectNotes(noteIds: string[], additive: boolean) {
    if (additive && noteIds.length === 0) return;
    const merged = additive ? [...this.selectedNoteIds, ...noteIds] : noteIds;
    this.setNoteSelection(merged);
    if (noteIds.length > 0) {
      this.selectionAnchorNoteId = noteIds.at(-1) ?? "";
    }
  }

  clearNoteSelection() {
    this.selectedNoteIds = [];
    this.selectionAnchorNoteId = "";
  }

  private setNoteSelection(noteIds: string[]) {
    const clip = this.selectedClip;
    if (!clip) {
      this.clearNoteSelection();
      return;
    }
    const seen = new Set<string>();
    const valid = noteIds.filter((id) => {
      if (seen.has(id)) return false;
      if (!clip.notes.some((note) => note.id === id)) return false;
      seen.add(id);
      return true;
    });
    this.selectedNoteIds = valid;
    if (!valid.includes(this.selectionAnchorNoteId)) {
      this.selectionAnchorNoteId = valid.at(-1) ?? "";
    }
  }

  async duplicateSelected() {
    const notes = this.selectedNotes;
    if (notes.length > 1) {
      await this.duplicateNotesGroup(notes);
      return;
    }
    if (this.selectedNotePath) {
      await this.duplicateNote(this.selectedNotePath);
      return;
    }
    if (this.selectedClipPath) {
      await this.duplicateClip(this.selectedClipPath);
    }
  }

  async deleteSelected() {
    const paths = this.selectedNotePaths;
    if (paths.length > 1) {
      await this.deleteNotes(paths);
      return;
    }
    if (paths.length === 1) {
      await this.deleteNote(paths[0] ?? "");
      return;
    }
    if (this.selectedClipPath) {
      await this.deleteClip(this.selectedClipPath);
    }
  }

  async quantizeSelected() {
    const paths = this.selectedNotePaths;
    if (paths.length === 0) return;
    const grid = this.secondsPerBeat / 4;
    if (paths.length === 1) {
      await this.quantizeNote(paths[0] ?? "", grid);
      return;
    }
    if (this.busy) return;
    this.busy = true;
    try {
      await this.run("note.quantize_many", { paths, grid });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.setStatus("status.notesQuantized", { count: paths.length });
    } catch (error) {
      this.fail("status.noteQuantizeFailed", error);
    } finally {
      this.busy = false;
    }
  }

  copySelected() {
    const notes = this.selectedNotes;
    if (notes.length > 0) {
      this.clipboard = {
        kind: "notes",
        notes: notes.map((note) => ({
          pitch: note.pitch,
          start: note.start,
          duration: note.duration,
          velocity: note.velocity,
        })),
      };
      if (notes.length === 1) {
        this.setStatus("status.noteCopied");
      } else {
        this.setStatus("status.notesCopied", { count: notes.length });
      }
      return;
    }
    if (this.selectedClip) {
      this.clipboard = {
        kind: "clip",
        name: this.selectedClip.name,
        duration: this.selectedClip.duration,
        notes: this.selectedClip.notes.map((note) => ({
          pitch: note.pitch,
          start: note.start,
          duration: note.duration,
          velocity: note.velocity,
        })),
      };
      this.setStatus("status.clipCopied");
      return;
    }
    this.setStatus("status.copyEmpty");
  }

  private async duplicateNotesGroup(notes: Note[]) {
    if (this.busy) return;
    const path = this.selectedClipPath;
    if (!path || notes.length === 0) return;
    const minStart = Math.min(...notes.map((note) => note.start));
    const maxEnd = Math.max(...notes.map((note) => note.start + note.duration));
    const offset = maxEnd - minStart;
    const drafts = notes.map((note) => ({
      pitch: note.pitch,
      start: note.start + offset,
      duration: note.duration,
      velocity: note.velocity,
    }));
    this.busy = true;
    try {
      const result = await this.run<{ note_ids: string[] }>("note.paste", {
        path,
        notes: drafts,
      });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.setNoteSelection(result.note_ids);
      this.setStatus("status.notesDuplicated", {
        count: result.note_ids.length,
      });
    } catch (error) {
      this.fail("status.noteDuplicateFailed", error);
    } finally {
      this.busy = false;
    }
  }

  private async deleteNotes(paths: string[]) {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.run("note.remove_many", { paths });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.clearNoteSelection();
      this.setStatus("status.notesRemoved", { count: paths.length });
    } catch (error) {
      this.fail("status.noteRemoveFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async pasteClipboard() {
    if (this.busy) return;
    const clipboard = this.clipboard;
    if (!clipboard) {
      this.setStatus("status.pasteEmpty");
      return;
    }
    this.busy = true;
    try {
      if (clipboard.kind === "notes") {
        const clip = this.selectedClip;
        const path = this.selectedClipPath;
        if (!clip || !path) {
          this.setStatus("status.pasteFailed", { message: "先选择一个片段" });
          return;
        }
        let base = 0;
        if (
          this.playhead >= clip.start &&
          this.playhead < clip.start + clip.duration
        ) {
          base = this.playhead - clip.start;
        }
        const result = await this.run<{ note_ids: string[] }>("note.paste", {
          path,
          notes: clipboard.notes.map((note) => ({
            ...note,
            start: note.start + base,
          })),
        });
        await this.refreshProject();
        await this.reloadIfPlaying();
        this.setNoteSelection(result.note_ids);
        this.setStatus("status.pasted");
      } else {
        const track = this.selectedTrack;
        if (!track) {
          this.setStatus("status.pasteFailed", { message: "先选择一条轨道" });
          return;
        }
        const start = Math.min(
          this.playhead,
          Math.max(0, this.duration - clipboard.duration),
        );
        const result = await this.run<{ clip_id: string }>("clip.paste", {
          track_id: track.id,
          name: `${clipboard.name} 副本`,
          start,
          duration: clipboard.duration,
          notes: clipboard.notes,
        });
        await this.refreshProject();
        await this.reloadIfPlaying();
        this.selectedClipId = result.clip_id;
        this.clearNoteSelection();
        this.setStatus("status.pasted");
      }
    } catch (error) {
      this.fail("status.pasteFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async openRecent(path: string) {
    if (this.busy) return;
    if (this.dirty) {
      this.pendingOpenPath = path;
      return;
    }
    await this.openPath(path);
  }

  async confirmPendingOpen() {
    const path = this.pendingOpenPath;
    if (!path) return;
    this.pendingOpenPath = null;
    await this.openPath(path);
  }

  cancelPendingOpen() {
    this.pendingOpenPath = null;
  }

  cancelClose() {
    this.closeRequested = false;
  }

  async quit() {
    try {
      await requestWindowClose();
    } catch (error) {
      this.fail("status.closeFailed", error);
    }
  }

  async saveAndClose() {
    if (this.busy) return;
    const saved = await this.saveProject();
    if (saved) await this.closeWindow();
  }

  async discardAndClose() {
    if (this.busy) return;
    try {
      await removeRecoveryFile();
    } catch (error) {
      this.fail("status.recoveryDiscardFailed", error);
      return;
    }
    this.dirty = false;
    await this.closeWindow();
  }

  async restoreRecovery() {
    if (this.busy) return;
    this.busy = true;
    try {
      if (!this.recoveryPath) this.recoveryPath = await recoveryPath();
      await this.run("project.open", { path: this.recoveryPath });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.projectPath = "";
      this.dirty = true;
      this.recoveryAvailable = false;
      this.setStatus("status.recovered");
    } catch (error) {
      this.fail("status.recoveryFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async discardRecovery() {
    if (this.busy) return;
    this.busy = true;
    try {
      await removeRecoveryFile();
      this.recoveryAvailable = false;
      this.recoveryPath = "";
      this.setStatus("status.recoveryDiscarded");
    } catch (error) {
      this.fail("status.recoveryDiscardFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async updateProject(patch: {
    tempo_bpm?: number;
    time_signature?: { numerator: number; denominator: number };
    length_bars?: number;
    loop_start?: number;
    loop_end?: number;
  }) {
    if (this.busy) return;
    const oldTempo = this.tempo;
    const oldBeat = this.playhead / Math.max(0.001, 60 / oldTempo);
    this.busy = true;
    try {
      await this.run("project.update", patch);
      await this.refreshProject();
      await this.reloadIfPlaying();
      const tempoChanged =
        patch.tempo_bpm !== undefined && Math.abs(this.tempo - oldTempo) > 1e-9;
      const nextPlayhead = tempoChanged
        ? Math.min(this.duration, oldBeat * (60 / this.tempo))
        : Math.min(this.playhead, this.duration);
      if (Math.abs(nextPlayhead - this.playhead) > 1e-9) {
        this.playhead = nextPlayhead;
        this.lastSeekAt = performance.now();
        void transportSeek(nextPlayhead).catch(() => {});
      }
      this.setStatus("status.projectUpdated");
    } catch (error) {
      this.fail("status.projectUpdateFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async addTrack(input: {
    name: string;
    source: "sine" | "silence" | "piano";
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

  async addFilter(trackId: string) {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.run("device.add", {
        track_id: trackId,
        kind: "filter.lowpass",
      });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.setStatus("status.filterAdded");
    } catch (error) {
      this.fail("status.filterAddFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async scanPlugins(): Promise<{
    directory: string;
    plugins: {
      id: string;
      name: string;
      vendor: string;
      version: string;
      path: string;
    }[];
    failures: { path: string; reason: string }[];
  }> {
    return this.run("plugin.scan", {});
  }

  async inspectPlugin(path: string, id: string): Promise<{
    path: string;
    id: string;
    parameters: {
      id: number;
      name: string;
      module: string;
      minimum: number;
      maximum: number;
      default: number;
      flags: number;
    }[];
    has_state: boolean;
  }> {
    return this.run("plugin.inspect", { path, id });
  }

  async addPluginNode(path: string, id: string): Promise<unknown> {
    if (!this.selectedTrackId) {
      throw new Error("先选择一条轨道");
    }
    return this.run("device.add", {
      track_id: this.selectedTrackId,
      kind: "plugin",
      plugin_path: path,
      plugin_id: id,
    });
  }

  get nodeGroups(): Record<string, NodeGroup> {
    return this.project?.node_groups ?? {};
  }

  get nodeTypes(): NodeTypeInfo[] {
    return this.nodeTypesState;
  }

  get audioNodeTypes(): NodeTypeInfo[] {
    return this.audioNodeTypesState;
  }

  async createGroup(name: string): Promise<string> {
    const data = await this.run<{ group_id: string }>("group.create", { name });
    await this.refreshProject();
    return data.group_id;
  }

  async groupFromNodes(
    sourceGroupId: string,
    nodeIds: string[],
    name?: string,
  ): Promise<{ group_id: string; node_id: string; name: string }> {
    const data = await this.run<{
      group_id: string;
      node_id: string;
      name: string;
    }>("group.from_nodes", {
      source_group_id: sourceGroupId,
      node_ids: nodeIds,
      ...(name ? { name } : {}),
    });
    await this.refreshProject();
    return data;
  }

  async ungroupNode(
    groupId: string,
    nodeId: string,
  ): Promise<{ group_id: string; node_ids: string[] }> {
    const data = await this.run<{
      group_id: string;
      node_ids: string[];
    }>("group.ungroup", { group_id: groupId, node_id: nodeId });
    await this.refreshProject();
    return data;
  }

  async deleteGroup(groupId: string): Promise<void> {
    await this.run("group.delete", { group_id: groupId });
    await this.refreshProject();
  }

  async renameGroup(groupId: string, name: string): Promise<void> {
    await this.run("group.rename", { group_id: groupId, name });
    await this.refreshProject();
  }

  async setGroupInterface(
    groupId: string,
    groupInterface: NodeGroup["interface"],
  ): Promise<unknown> {
    const result = await this.run("group.interface", {
      group_id: groupId,
      interface: groupInterface,
    });
    await this.refreshProject();
    return result;
  }

  async setGroupVoice(
    groupId: string,
    voice: {
      nodes: Record<string, unknown>;
      connections: { from: { node: string; port: string }; to: { node: string; port: string } }[];
    },
  ): Promise<unknown> {
    const result = await this.run("group.voice", { group_id: groupId, voice });
    await this.refreshProject();
    return result;
  }

  async setMasterPlugin(path: string, id: string): Promise<unknown> {
    return this.run("plugin.master", { path, id });
  }

  async clearMasterPlugin(): Promise<unknown> {
    return this.run("plugin.master_off", {});
  }

  async scanExtensions(): Promise<{
    directory: string;
    extensions: {
      id: string;
      name: string;
      version: string;
      api_version: number;
      directory: string;
      commands: { id: string; name: string; executable: string }[];
    }[];
    failures: { path: string; reason: string }[];
  }> {
    return this.run("extension.list", {});
  }

  async callExtension(
    extension: string,
    command: string,
    args: Record<string, unknown> = {},
  ): Promise<unknown> {
    const data = await this.run<{ result: unknown }>("extension.call", {
      extension,
      command,
      args,
    });
    return data.result;
  }

  async setNodeFlags(
    trackId: string,
    nodeId: string,
    flags: { muted?: boolean; collapsed?: boolean },
  ): Promise<void> {
    await this.run("node.flags", {
      track_id: trackId,
      node_id: nodeId,
      ...flags,
    });
    await this.refreshProject();
  }

  async addGraphNode(
    trackId: string,
    kind: string,
    x: number,
    y: number,
  ): Promise<string | undefined> {
    if (this.busy) return undefined;
    this.busy = true;
    try {
      const data = await this.run<{ node_id: string }>("node.add", {
        track_id: trackId,
        kind,
        position: [x, y],
      });
      await this.refreshProject();
      this.setStatus("status.nodeAdded");
      return data.node_id;
    } catch (error) {
      this.fail("status.nodeAddFailed", error);
      return undefined;
    } finally {
      this.busy = false;
    }
  }

  async duplicateNodes(
    paths: string[],
  ): Promise<{ source: string; node_id: string }[] | undefined> {
    if (this.busy || paths.length === 0) return undefined;
    this.busy = true;
    try {
      const data = await this.run<{
        nodes: { source: string; node_id: string }[];
      }>("node.duplicate", { paths, offset: [40, 40] });
      await this.refreshProject();
      this.setStatus("status.nodesDuplicated");
      return data.nodes;
    } catch (error) {
      this.fail("status.nodesDuplicateFailed", error);
      return undefined;
    } finally {
      this.busy = false;
    }
  }

  async addMix(trackId: string) {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.run("device.add", { track_id: trackId, kind: "mix" });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.setStatus("status.mixAdded");
    } catch (error) {
      this.fail("status.mixAddFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async addAdsr(trackId: string) {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.run("device.add", { track_id: trackId, kind: "amp.adsr" });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.setStatus("status.adsrAdded");
    } catch (error) {
      this.fail("status.adsrAddFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async changeDeviceParam(
    trackId: string,
    deviceId: string,
    name: string,
    value: number,
  ) {
    try {
      await this.run("param.set", {
        path: `/tracks/${trackId}/devices/${deviceId}/params/${name}`,
        value,
      });
      await this.reloadIfPlaying();
      await this.refreshProject();
    } catch (error) {
      this.fail("status.deviceParamFailed", error);
    }
  }

  async moveDevice(
    trackId: string,
    nodeId: string,
    beforeNodeId: string,
  ) {
    try {
      await this.run("node.move", {
        path: `/tracks/${trackId}/nodes/${nodeId}`,
        before: `/tracks/${trackId}/nodes/${beforeNodeId}`,
      });
      await this.reloadIfPlaying();
      await this.refreshProject();
      this.setStatus("status.nodeMoved");
    } catch (error) {
      this.fail("status.nodeMoveFailed", error);
    }
  }

  async connectNodes(
    trackId: string,
    fromNodeId: string,
    fromPort: string,
    toNodeId: string,
    toPort: string,
  ) {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.run("node.connect", {
        from: `/tracks/${trackId}/nodes/${fromNodeId}`,
        from_port: fromPort,
        to: `/tracks/${trackId}/nodes/${toNodeId}`,
        to_port: toPort,
      });
      await this.reloadIfPlaying();
      await this.refreshProject();
      this.setStatus("status.nodeConnected");
    } catch (error) {
      this.fail("status.nodeConnectFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async disconnectNodes(
    trackId: string,
    fromNodeId: string,
    fromPort: string,
    toNodeId: string,
    toPort: string,
  ) {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.run("node.disconnect", {
        from: `/tracks/${trackId}/nodes/${fromNodeId}`,
        from_port: fromPort,
        to: `/tracks/${trackId}/nodes/${toNodeId}`,
        to_port: toPort,
      });
      await this.reloadIfPlaying();
      await this.refreshProject();
      this.setStatus("status.nodeDisconnected");
    } catch (error) {
      this.fail("status.nodeDisconnectFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async placeNodes(
    trackId: string,
    nodes: { id: string; x: number; y: number }[],
  ) {
    if (this.busy || nodes.length === 0) return;
    this.busy = true;
    try {
      await this.run("node.position", {
        positions: nodes.map((node) => ({
          path: `/tracks/${trackId}/nodes/${node.id}`,
          x: node.x,
          y: node.y,
        })),
      });
      await this.refreshProject();
      this.setStatus("status.nodesPlaced");
    } catch (error) {
      this.fail("status.nodesPlaceFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async deleteNodes(paths: string[]) {
    if (this.busy || paths.length === 0) return;
    this.busy = true;
    try {
      await this.run("node.remove_many", { paths });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.setStatus("status.nodesRemoved");
    } catch (error) {
      this.fail("status.nodesRemoveFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async deleteNode(path: string) {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.run("node.remove", { path });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.setStatus("status.deviceRemoved");
    } catch (error) {
      this.fail("status.deviceRemoveFailed", error);
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
      this.clearNoteSelection();
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
      this.clearNoteSelection();
      const id = path.split("/").pop() ?? path;
      this.setStatus("status.clipRemoved", { id });
    } catch (error) {
      this.fail("status.clipRemoveFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async duplicateClip(path: string) {
    if (this.busy) return;
    this.busy = true;
    try {
      const result = await this.run<{ clip_id: string }>("clip.duplicate", {
        path,
      });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.selectedClipId = result.clip_id;
      this.clearNoteSelection();
      this.setStatus("status.clipDuplicated", { id: result.clip_id });
    } catch (error) {
      this.fail("status.clipDuplicateFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async moveClip(path: string, trackId: string, start: number) {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.run("clip.move", { path, track_id: trackId, start });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.selectedTrackId = trackId;
      this.selectedClipId = path.split("/").pop() ?? this.selectedClipId;
      this.clearNoteSelection();
      this.setStatus("status.clipMoved", { id: this.selectedClipId });
    } catch (error) {
      this.fail("status.clipMoveFailed", error);
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
      this.selectNote(result.note_id);
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
      if (this.selectedNoteIds.includes(id)) {
        this.setNoteSelection(
          this.selectedNoteIds.filter((noteId) => noteId !== id),
        );
      }
      this.setStatus("status.noteRemoved", { id });
    } catch (error) {
      this.fail("status.noteRemoveFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async duplicateNote(path: string) {
    if (this.busy) return;
    this.busy = true;
    try {
      const result = await this.run<{ note_id: string }>("note.duplicate", {
        path,
      });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.selectNote(result.note_id);
      this.setStatus("status.noteDuplicated", { id: result.note_id });
    } catch (error) {
      this.fail("status.noteDuplicateFailed", error);
    } finally {
      this.busy = false;
    }
  }

  async quantizeNote(path: string, grid: number) {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.run("note.quantize", { path, grid });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.setStatus("status.noteQuantized", {
        id: path.split("/").pop() ?? path,
      });
    } catch (error) {
      this.fail("status.noteQuantizeFailed", error);
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
      const result = await this.run<{ undone: string[] }>("undo", {
        steps: 1,
      });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.ensureVisibleClip(result.undone);
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
      const result = await this.run<{ redone: string[] }>("redo", {
        steps: 1,
      });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.ensureVisibleClip(result.redone);
      this.setStatus("status.redone");
    } catch (error) {
      this.fail("status.redoFailed", error);
    } finally {
      this.busy = false;
    }
  }

  private ensureVisibleClip(commands: string[]) {
    const structural = commands.some(
      (cmd) =>
        cmd === "clip.move" || cmd === "clip.remove" || cmd === "track.remove",
    );
    if (!structural || this.selectedClip) return;
    const track = this.tracks.find((item) => item.clips.length > 0);
    if (track?.clips[0]) this.selectClip(track.id, track.clips[0].id);
  }

  async play() {
    if (this.busy || this.playing) return;
    try {
      await setMasterGain(this.gain / 100);
      await transportPlay();
      this.playing = true;
      this.lastTransportPosition = -1;
      this.lastTransportProgressAt = performance.now();
      this.audioRecoveryAttempts = 0;
      this.setStatus("status.playing");
    } catch (error) {
      this.fail("status.playFailed", error);
    }
  }

  async setMetronome(enabled: boolean) {
    try {
      await this.run("transport.metronome", { enabled });
      this.metronome = enabled;
      this.setStatus("status.metronomeChanged");
    } catch (error) {
      this.fail("status.metronomeFailed", error);
    }
  }

  async stop() {
    try {
      this.cancelSeek();
      await transportStop();
      this.playing = false;
      this.lastTransportPosition = -1;
      this.audioRecoveryAttempts = 0;
      this.setStatus("status.stopped");
    } catch (error) {
      this.fail("status.stopFailed", error);
    }
  }

  private async recoverAudio() {
    if (this.recoveringAudio) return;
    this.recoveringAudio = true;
    this.audioRecoveryAttempts += 1;
    this.setStatus("status.audioRecovering");
    try {
      this.cancelSeek();
      await transportStop();
      await setMasterGain(this.gain / 100);
      await transportPlay();
      this.playing = true;
      this.lastTransportPosition = -1;
      this.lastTransportProgressAt = performance.now();
      this.setStatus("status.audioRecovered");
    } catch (error) {
      this.fail("status.audioLost", error);
    } finally {
      this.recoveringAudio = false;
    }
  }

  seek(seconds: number) {
    this.playhead = seconds;
    this.lastSeekAt = performance.now();
    const now = performance.now();
    const wait = 30 - (now - this.lastSeekSentAt);
    if (wait <= 0) {
      this.cancelPendingSeek();
      this.sendSeek(seconds);
      return;
    }
    this.pendingSeekSeconds = seconds;
    if (this.seekTimer === null) {
      this.seekTimer = window.setTimeout(() => {
        this.seekTimer = null;
        const pending = this.pendingSeekSeconds;
        this.pendingSeekSeconds = null;
        if (pending !== null) this.sendSeek(pending);
      }, wait);
    }
  }

  private sendSeek(seconds: number) {
    this.lastSeekSentAt = performance.now();
    void transportSeek(seconds).catch(() => {});
  }

  private cancelPendingSeek() {
    if (this.seekTimer !== null) {
      window.clearTimeout(this.seekTimer);
      this.seekTimer = null;
    }
    this.pendingSeekSeconds = null;
  }

  private cancelSeek() {
    this.cancelPendingSeek();
    this.lastSeekSentAt = 0;
  }

  async render() {
    if (this.busy) return;
    this.busy = true;
    try {
      this.cancelSeek();
      await transportStop();
      this.playing = false;
      this.lastTransportPosition = -1;
      this.audioRecoveryAttempts = 0;
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

  async saveProject(): Promise<boolean> {
    if (this.busy) return false;
    if (this.projectPath) return this.writeProject(this.projectPath);
    return this.saveProjectAs();
  }

  async saveProjectAs(): Promise<boolean> {
    if (this.busy) return false;
    let path: string | null;
    try {
      path = await pickProjectPath("save");
    } catch (error) {
      this.fail("status.saveFailed", error);
      return false;
    }
    if (!path) return false;
    return this.writeProject(path);
  }

  private async writeProject(path: string): Promise<boolean> {
    if (this.busy) return false;
    this.busy = true;
    try {
      await this.run("project.save", { path });
      this.projectPath = path;
      this.rememberRecent(path);
      await this.clearRecovery();
      this.setStatus("status.saved", { file: fileName(path) });
      return true;
    } catch (error) {
      this.fail("status.saveFailed", error);
      return false;
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
    if (this.dirty) {
      this.pendingOpenPath = path;
      return;
    }
    await this.openPath(path);
  }

  private async openPath(path: string) {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.run("project.open", { path });
      await this.refreshProject();
      await this.reloadIfPlaying();
      this.projectPath = path;
      this.rememberRecent(path);
      await this.clearRecovery();
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

  private rememberRecent(path: string) {
    const entry: RecentProject = {
      path,
      name: fileName(path),
      savedAt: Date.now(),
    };
    this.recentProjects = [
      entry,
      ...this.recentProjects.filter((item) => item.path !== path),
    ].slice(0, MAX_RECENT);
    try {
      localStorage.setItem(RECENT_KEY, JSON.stringify(this.recentProjects));
    } catch {
      // 最近工程是本地偏好，存储失败不影响当前会话。
    }
  }

  private markDirty() {
    this.dirty = true;
    this.scheduleRecovery();
  }

  private scheduleRecovery() {
    if (this.recoveryTimer !== null) {
      window.clearTimeout(this.recoveryTimer);
    }
    this.recoveryTimer = window.setTimeout(() => {
      this.recoveryTimer = null;
      void this.writeRecovery();
    }, RECOVERY_DELAY_MS);
  }

  private async writeRecovery() {
    if (!this.dirty || this.recovering) return;
    if (!this.recoveryPath) {
      try {
        this.recoveryPath = await recoveryPath();
      } catch {
        return;
      }
    }
    const token = this.recoveryToken;
    const path = this.recoveryPath;
    this.recovering = true;
    try {
      await call("project.save", { path });
    } catch {
      // 自动恢复失败不覆盖用户当前操作，下一次修改会重试。
    } finally {
      this.recovering = false;
    }
    if (token !== this.recoveryToken && !this.dirty) {
      try {
        await removeRecoveryFile();
      } catch {
        // 已保存的工程不需要恢复文件，删除失败留给下次清理。
      }
    }
  }

  private async clearRecovery() {
    this.recoveryToken += 1;
    this.dirty = false;
    this.recoveryAvailable = false;
    this.lastProjectSnapshot = JSON.stringify(this.project);
    if (this.recoveryTimer !== null) {
      window.clearTimeout(this.recoveryTimer);
      this.recoveryTimer = null;
    }
    try {
      await removeRecoveryFile();
    } catch {
      // 恢复文件清理失败不影响已经保存的工程。
    }
  }

  private async checkRecovery() {
    try {
      this.recoveryPath = await recoveryPath();
      this.recoveryAvailable = await recoveryExists();
    } catch {
      this.recoveryAvailable = false;
    }
  }

  private async initCloseGuard() {
    if (this.closeGuardRegistered) return;
    this.closeGuardRegistered = true;
    try {
      this.unlistenClose = await onCloseRequested((event) => {
        if (!this.dirty) return;
        event.preventDefault();
        this.closeRequested = true;
      });
    } catch (error) {
      this.closeGuardRegistered = false;
      this.fail("status.closeGuardFailed", error);
    }
  }

  private async closeWindow() {
    try {
      await destroyWindow();
    } catch (error) {
      this.fail("status.closeFailed", error);
    }
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
    const result = await call<T>(cmd, args);
    if (MUTATING_COMMANDS.has(cmd)) this.markDirty();
    return result;
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
    while (this.projectPolling) {
      await new Promise<void>((resolve) => window.setTimeout(resolve, 8));
    }
    this.projectPolling = true;
    try {
      const state: ProjectState = await projectState();
      this.project = state.project;
      this.ensureSelection();
      const snapshot = JSON.stringify(state.project);
      if (
        this.lastProjectSnapshot !== "" &&
        snapshot !== this.lastProjectSnapshot &&
        !this.dirty
      ) {
        this.markDirty();
      }
      this.lastProjectSnapshot = snapshot;
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
    const clip = clips.find((item) => item.id === this.selectedClipId);
    const notes = clip?.notes ?? [];
    if (this.selectedNoteIds.length > 0) {
      const valid = this.selectedNoteIds.filter((id) =>
        notes.some((note) => note.id === id),
      );
      if (valid.length !== this.selectedNoteIds.length) {
        this.selectedNoteIds = valid;
      }
    }
    if (
      this.selectionAnchorNoteId !== "" &&
      !notes.some((note) => note.id === this.selectionAnchorNoteId)
    ) {
      this.selectionAnchorNoteId = this.selectedNoteIds.at(-1) ?? "";
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
      this.lastProjectSnapshot = JSON.stringify(this.project);
      const description = await call<{ commands: { id: string }[] }>(
        "describe",
      );
      this.commandCount = description.commands.length;
      const nodeTypes = await call<{
        types: NodeTypeInfo[];
        audio_types: NodeTypeInfo[];
      }>("node.types");
      this.nodeTypesState = nodeTypes.types;
      this.audioNodeTypesState = nodeTypes.audio_types;
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
      this.metronome = snapshot.metronome;
      if (snapshot.playing) {
        const now = performance.now();
        if (
          this.lastTransportPosition < 0 ||
          Math.abs(snapshot.position - this.lastTransportPosition) > 1e-9
        ) {
          this.lastTransportPosition = snapshot.position;
          this.lastTransportProgressAt = now;
          this.audioRecoveryAttempts = 0;
        } else if (
          now - this.lastTransportProgressAt > 1500 &&
          !this.recoveringAudio &&
          this.audioRecoveryAttempts < 2
        ) {
          void this.recoverAudio();
        }
        if (now - this.lastSeekAt > 200) {
          this.playhead = snapshot.position;
        }
      } else {
        this.lastTransportPosition = -1;
        this.audioRecoveryAttempts = 0;
      }
    } catch {
      // 轮询失败时保持现状，不打断交互。
    } finally {
      this.polling = false;
    }
  }
}

export const session = new SessionStore();
