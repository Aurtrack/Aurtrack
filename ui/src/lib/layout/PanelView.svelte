<script lang="ts">
  import ClipEditor from "../components/ClipEditor.svelte";
  import CommandLog from "../components/CommandLog.svelte";
  import PianoRoll from "../components/PianoRoll.svelte";
  import Timeline from "../components/Timeline.svelte";
  import TrackInspector from "../components/TrackInspector.svelte";
  import TrackList from "../components/TrackList.svelte";
  import TransportPanel from "../components/TransportPanel.svelte";
  import type { PanelKind } from "../layout.svelte";
  import { session } from "../session.svelte";

  let { kind }: { kind: PanelKind } = $props();
</script>

{#if kind === "timeline"}
  <Timeline
    tracks={session.timelineTracks}
    duration={session.duration}
    playhead={session.playhead}
    onSeek={(seconds: number) => session.seek(seconds)}
    onSelectClip={(trackId: string, clipId: string) =>
      session.selectClip(trackId, clipId)}
    onUpdateClip={(path: string, patch: { start?: number; duration?: number }) =>
      void session.updateClip(path, patch)}
    onUpdateNote={(path: string, patch: { start?: number; duration?: number }) =>
      void session.updateNote(path, patch)}
  />
{:else if kind === "piano"}
  <PianoRoll
    trackId={session.selectedTrack?.id ?? null}
    clip={session.selectedClip}
    busy={session.busy}
    onAddNote={(path, input) => void session.addNote(path, input)}
    onUpdateNote={(path, patch) => void session.updateNote(path, patch)}
    onRemoveNote={(path: string) => void session.deleteNote(path)}
  />
{:else if kind === "tracks"}
  <TrackList
    tracks={session.tracks}
    selectedId={session.selectedTrack?.id ?? ""}
    busy={session.busy}
    onSelect={(id: string) => session.selectTrack(id)}
    onAdd={(input) => void session.addTrack(input)}
    onRemove={(id: string) => void session.deleteTrack(id)}
  />
{:else if kind === "clips"}
  <ClipEditor
    track={session.selectedTrack}
    busy={session.busy}
    selectedClipId={session.selectedClipId}
    onSelectClip={(id: string) => session.selectClipId(id)}
    onAddClip={(trackId, input) => void session.addClip(trackId, input)}
    onRemoveClip={(path: string) => void session.deleteClip(path)}
    onAddNote={(path, input) => void session.addNote(path, input)}
    onRemoveNote={(path: string) => void session.deleteNote(path)}
  />
{:else if kind === "inspector"}
  <TrackInspector
    track={session.selectedTrack}
    busy={session.busy}
    onAddGain={(trackId: string) => void session.addGain(trackId)}
    onRemoveDevice={(path: string) => void session.deleteDevice(path)}
    onGainChange={(trackId, deviceId, value) =>
      void session.changeDeviceGain(trackId, deviceId, value)}
  />
{:else if kind === "transport"}
  <TransportPanel
    playing={session.playing}
    busy={session.busy}
    gain={session.gain}
    status={session.statusText}
    fileName={session.projectFile}
    dirty={session.dirty}
    commandCount={session.commandCount}
    onPlay={() => void session.play()}
    onStop={() => void session.stop()}
    onGainChange={(value: number) => session.onGainChange(value)}
    onRender={() => void session.render()}
    onSave={() => void session.saveProject()}
    onOpen={() => void session.openProject()}
    onUndo={() => void session.undo()}
    onRedo={() => void session.redo()}
  />
{:else}
  <CommandLog lines={session.logLines} />
{/if}
