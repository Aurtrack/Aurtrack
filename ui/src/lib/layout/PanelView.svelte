<script lang="ts">
  import ClipEditor from "../components/ClipEditor.svelte";
  import CommandLog from "../components/CommandLog.svelte";
  import NodeGraph from "../components/NodeGraph.svelte";
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
    onUpdateLoop={(start: number, end: number) =>
      void session.updateProject({ loop_start: start, loop_end: end })}
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
    onAddAdsr={(trackId: string) => void session.addAdsr(trackId)}
    onAddFilter={(trackId: string) => void session.addFilter(trackId)}
    onMoveDevice={(trackId, nodeId, beforeNodeId) =>
      void session.moveDevice(trackId, nodeId, beforeNodeId)}
    onRemoveDevice={(path: string) => void session.deleteDevice(path)}
    onGainChange={(trackId, deviceId, value) =>
      void session.changeDeviceGain(trackId, deviceId, value)}
    onParamChange={(trackId, deviceId, name, value) =>
      void session.changeDeviceParam(trackId, deviceId, name, value)}
  />
{:else if kind === "graph"}
  <NodeGraph
    track={session.selectedTrack}
    busy={session.busy}
    onAddNode={(trackId, kind, x, y, group) =>
      session.addGraphNode(trackId, kind, x, y, group)}
    onConnect={(trackId, fromNodeId, fromPort, toNodeId, toPort) =>
      session.connectNodes(
        trackId,
        fromNodeId,
        fromPort,
        toNodeId,
        toPort,
      )}
    onDisconnect={(trackId, fromNodeId, fromPort, toNodeId, toPort) =>
      session.disconnectNodes(
        trackId,
        fromNodeId,
        fromPort,
        toNodeId,
        toPort,
      )}
    onPlace={(trackId, nodes) => session.placeNodes(trackId, nodes)}
    onDuplicate={(paths) => session.duplicateNodes(paths)}
    onRemove={(path: string) => void session.deleteNode(path)}
    onRemoveMany={(paths) => void session.deleteNodes(paths)}
    onParamChange={(trackId, nodeId, name, value) =>
      void session.changeDeviceParam(trackId, nodeId, name, value)}
  />
{:else if kind === "transport"}
  <TransportPanel
    playing={session.playing}
    busy={session.busy}
    metronome={session.metronome}
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
    onMetronomeChange={(enabled) => void session.setMetronome(enabled)}
    onTimebaseChange={(patch) => void session.updateProject(patch)}
  />
{:else}
  <CommandLog lines={session.logLines} />
{/if}
