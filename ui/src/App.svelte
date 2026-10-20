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
  import { session } from "./lib/session.svelte";

  onMount(() => {
    session.start();
    return () => session.dispose();
  });
</script>

<main class="sheet">
  <Header audioState={session.audioState} sampleRate={session.sampleRate} />
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
  <div class="split">
    <div class="column">
      <TrackList
        tracks={session.tracks}
        selectedId={session.selectedTrack?.id ?? ""}
        busy={session.busy}
        onSelect={(id: string) => session.selectTrack(id)}
        onAdd={(input) => void session.addTrack(input)}
        onRemove={(id: string) => void session.deleteTrack(id)}
      />
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
    </div>
    <div class="column">
      <TrackInspector
        track={session.selectedTrack}
        busy={session.busy}
        onAddGain={(trackId: string) => void session.addGain(trackId)}
        onRemoveDevice={(path: string) => void session.deleteDevice(path)}
        onGainChange={(trackId, deviceId, value) =>
          void session.changeDeviceGain(trackId, deviceId, value)}
      />
      <TransportPanel
        playing={session.playing}
        busy={session.busy}
        gain={session.gain}
        status={session.statusText}
        fileName={session.projectFile}
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
    </div>
  </div>
  <PianoRoll
    trackId={session.selectedTrack?.id ?? null}
    clip={session.selectedClip}
    busy={session.busy}
    onAddNote={(path, input) => void session.addNote(path, input)}
    onUpdateNote={(path, patch) => void session.updateNote(path, patch)}
    onRemoveNote={(path: string) => void session.deleteNote(path)}
  />
  <CommandLog lines={session.logLines} />
</main>
