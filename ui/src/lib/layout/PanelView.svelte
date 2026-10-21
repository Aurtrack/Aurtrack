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
    barSeconds={session.barSeconds}
    beatsPerBar={
      (session.timeSignature.numerator * 4) /
      session.timeSignature.denominator
    }
    loopStart={session.loopStart}
    loopEnd={session.loopEnd}
    onSeek={(seconds: number) => session.seek(seconds)}
    onSelectClip={(trackId: string, clipId: string) =>
      session.selectClip(trackId, clipId)}
    onUpdateClip={(path: string, patch: { start?: number; duration?: number }) =>
      void session.updateClip(path, patch)}
    onUpdateNote={(path: string, patch: { start?: number; duration?: number }) =>
      void session.updateNote(path, patch)}
    onMoveClip={(path: string, trackId: string, start: number) =>
      void session.moveClip(path, trackId, start)}
  />
{:else if kind === "piano"}
  <PianoRoll
    trackId={session.selectedTrack?.id ?? null}
    clip={session.selectedClip}
    busy={session.busy}
    barSeconds={session.barSeconds}
    beatsPerBar={
      (session.timeSignature.numerator * 4) /
      session.timeSignature.denominator
    }
    selectedNoteIds={session.selectedNoteIds}
    onSelectNote={(noteId, mode) => session.selectNote(noteId, mode)}
    onSelectNotes={(noteIds, additive) =>
      session.selectNotes(noteIds, additive)}
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
    barSeconds={session.barSeconds}
    beatsPerBar={
      (session.timeSignature.numerator * 4) /
      session.timeSignature.denominator
    }
    selectedClipId={session.selectedClipId}
    selectedNoteIds={session.selectedNoteIds}
    onSelectClip={(id: string) => session.selectClipId(id)}
    onSelectNote={(id, mode) => session.selectNote(id, mode)}
    onAddClip={(trackId, input) => void session.addClip(trackId, input)}
    onRemoveClip={(path: string) => void session.deleteClip(path)}
    onDuplicateClip={(path: string) => void session.duplicateClip(path)}
    onAddNote={(path, input) => void session.addNote(path, input)}
    onRemoveNote={(path: string) => void session.deleteNote(path)}
    onDuplicateSelected={() => void session.duplicateSelected()}
    onQuantizeSelected={() => void session.quantizeSelected()}
    onDeleteSelected={() => void session.deleteSelected()}
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
    tempo={session.tempo}
    timeSignature={session.timeSignature}
    lengthBars={session.lengthBars}
    loopStart={session.loopStart}
    loopEnd={session.loopEnd}
    barSeconds={session.barSeconds}
    onPlay={() => void session.play()}
    onStop={() => void session.stop()}
    onGainChange={(value: number) => session.onGainChange(value)}
    onTimebaseChange={(patch) => void session.updateProject(patch)}
  />
{:else}
  <CommandLog lines={session.logLines} />
{/if}
