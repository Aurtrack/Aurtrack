<script lang="ts">
  import type { Track } from "../api";
  import { formatNumber, t } from "../i18n.svelte";
  import {
    beatsToSeconds,
    formatBarBeat,
    secondsToBeats,
  } from "../music";
  import type { NoteSelectMode } from "../session.svelte";
  import NumberField from "./NumberField.svelte";

  interface Props {
    track: Track | null;
    busy: boolean;
    barSeconds: number;
    beatsPerBar: number;
    selectedClipId: string;
    onSelectClip: (id: string) => void;
    onAddClip: (
      trackId: string,
      input: { name: string; start: number; duration: number },
    ) => void;
    onRemoveClip: (path: string) => void;
    onAddNote: (
      path: string,
      input: {
        pitch: number;
        start: number;
        duration: number;
        velocity: number;
      },
    ) => void;
    onRemoveNote: (path: string) => void;
    selectedNoteIds: string[];
    onSelectNote: (noteId: string, mode: NoteSelectMode) => void;
    onDuplicateClip: (path: string) => void;
    onDuplicateSelected: () => void;
    onQuantizeSelected: () => void;
    onDeleteSelected: () => void;
  }

  let {
    track,
    busy,
    barSeconds,
    beatsPerBar,
    selectedClipId,
    onSelectClip,
    onAddClip,
    onRemoveClip,
    onAddNote,
    onRemoveNote,
    selectedNoteIds,
    onSelectNote,
    onDuplicateClip,
    onDuplicateSelected,
    onQuantizeSelected,
    onDeleteSelected,
  }: Props = $props();

  let clipName = $state("");
  let clipStart = $state(0);
  let clipDuration = $state(4);
  let notePitch = $state(60);
  let noteStart = $state(0);
  let noteDuration = $state(1);
  let noteVelocity = $state(80);

  let clips = $derived(track?.clips ?? []);
  let selectedClip = $derived(
    clips.find((clip) => clip.id === selectedClipId) ?? clips[0] ?? null,
  );
  let selectedCount = $derived(
    selectedClip
      ? selectedClip.notes.filter((note) => selectedNoteIds.includes(note.id))
          .length
      : 0,
  );

  function selectMode(event: MouseEvent): NoteSelectMode {
    if (event.ctrlKey || event.metaKey) return "toggle";
    if (event.shiftKey) return "range";
    return "replace";
  }

  const NOTE_NAMES = [
    "C",
    "C#",
    "D",
    "D#",
    "E",
    "F",
    "F#",
    "G",
    "G#",
    "A",
    "A#",
    "B",
  ];

  function noteName(pitch: number): string {
    const name = NOTE_NAMES[pitch % 12] ?? "?";
    return `${name}${Math.floor(pitch / 12) - 1}`;
  }

  function position(seconds: number): string {
    return formatBarBeat(seconds, barSeconds, beatsPerBar);
  }

  function beatCount(seconds: number): string {
    return formatNumber(secondsToBeats(seconds, barSeconds, beatsPerBar), 2);
  }

  function submitClip(trackId: string) {
    const trimmed = clipName.trim();
    onAddClip(trackId, {
      name:
        trimmed.length > 0
          ? trimmed
          : t("clips.defaultName", { index: clips.length + 1 }),
      start: beatsToSeconds(Number(clipStart), barSeconds, beatsPerBar),
      duration: beatsToSeconds(
        Number(clipDuration),
        barSeconds,
        beatsPerBar,
      ),
    });
    clipName = "";
  }

  function submitNote(trackId: string, clipId: string) {
    onAddNote(`/tracks/${trackId}/clips/${clipId}`, {
      pitch: Number(notePitch),
      start: beatsToSeconds(Number(noteStart), barSeconds, beatsPerBar),
      duration: beatsToSeconds(
        Number(noteDuration),
        barSeconds,
        beatsPerBar,
      ),
      velocity: Number(noteVelocity) / 100,
    });
  }
</script>

<section class="section">
  <div class="section-head">
    <span class="index">03</span>
    <span class="title">{t("clips.title")}</span>
    <span class="rule"></span>
    <span class="meta">{t("clips.total", { count: clips.length })}</span>
  </div>

  {#if track}
    {@const owner = track}
    <div class="clip-form">
      <label class="field name">
        <span>{t("clips.namePlaceholder")}</span>
        <input class="text-input" bind:value={clipName} />
      </label>
      <label class="field narrow">
        <span>{t("clips.start")}</span>
        <NumberField
          value={clipStart}
          min={0}
          step={0.25}
          precision={2}
          onchange={(value) => (clipStart = value)}
        />
      </label>
      <label class="field narrow">
        <span>{t("clips.duration")}</span>
        <NumberField
          value={clipDuration}
          min={0.25}
          step={0.25}
          precision={2}
          onchange={(value) => (clipDuration = value)}
        />
      </label>
      <button class="primary" disabled={busy} onclick={() => submitClip(owner.id)}>
        {t("clips.add")}
      </button>
    </div>

    <div class="clip-list">
      {#if clips.length === 0}
        <p class="empty">{t("clips.empty")}</p>
      {/if}
      {#each clips as clip (clip.id)}
        <div class="clip-row" class:selected={clip.id === selectedClip?.id}>
          <button
            class="clip-select"
            onclick={() => onSelectClip(clip.id)}
          >
            <span class="track-id">{clip.id}</span>
            <span class="track-name">{clip.name}</span>
            <span class="track-meta">
              {t("clips.range", {
                start: position(clip.start),
                end: position(clip.start + clip.duration),
                notes: clip.notes.length,
              })}
            </span>
          </button>
          <button
            class="track-remove"
            title={t("clips.removeTitle")}
            disabled={busy}
            onclick={() =>
              onRemoveClip(`/tracks/${owner.id}/clips/${clip.id}`)}
          >
            ×
          </button>
        </div>
      {/each}
    </div>

    {#if selectedClip}
      {@const current = selectedClip}
      <div class="clip-notes-head">
        <span class="track-meta">{t("notes.title")} · {current.id}</span>
        <span class="track-meta">
          {t("notes.total", { count: current.notes.length })}
        </span>
        <button
          class="mini-button"
          disabled={busy}
          onclick={() =>
            onDuplicateClip(`/tracks/${owner.id}/clips/${current.id}`)}
        >
          {t("clips.duplicate")}
        </button>
      </div>

      <div class="note-list">
        {#if current.notes.length === 0}
          <p class="empty">{t("notes.empty")}</p>
        {/if}
        {#each current.notes as note (note.id)}
          <div
            class="note-row"
            class:selected={selectedNoteIds.includes(note.id)}
          >
            <button
              class="note-select"
              disabled={busy}
              onclick={(event) => onSelectNote(note.id, selectMode(event))}
            >
              <span class="device-id">{note.id}</span>
              <span class="note-pitch">{noteName(note.pitch)}</span>
              <span class="track-meta">
                {t("notes.meta", {
                  start: beatCount(note.start),
                  duration: beatCount(note.duration),
                  velocity: formatNumber(note.velocity * 100),
                })}
              </span>
            </button>
            <button
              class="track-remove"
              title={t("notes.removeTitle")}
              disabled={busy}
              onclick={() =>
                onRemoveNote(
                  `/tracks/${owner.id}/clips/${current.id}/notes/${note.id}`,
                )}
            >
              ×
            </button>
          </div>
        {/each}
      </div>

      {#if selectedCount > 0}
        <div class="note-actions">
          <span class="track-meta">
            {t("notes.selected", { count: selectedCount })}
          </span>
          <button
            class="mini-button"
            disabled={busy}
            onclick={() => onDuplicateSelected()}
          >
            {t("notes.duplicate")}
          </button>
          <button
            class="mini-button"
            disabled={busy}
            onclick={() => onQuantizeSelected()}
          >
            {t("notes.quantize16")}
          </button>
          <button
            class="mini-button"
            disabled={busy}
            onclick={() => onDeleteSelected()}
          >
            {t("notes.deleteSelected")}
          </button>
        </div>
      {/if}

      <div class="note-form">
        <label class="field narrow">
          <span>{t("notes.pitch")}</span>
          <NumberField
            value={notePitch}
            min={0}
            max={127}
            step={1}
            precision={0}
            onchange={(value) => (notePitch = value)}
          />
        </label>
        <label class="field narrow">
          <span>{t("notes.start")}</span>
          <NumberField
            value={noteStart}
            min={0}
            step={0.25}
            precision={2}
            onchange={(value) => (noteStart = value)}
          />
        </label>
        <label class="field narrow">
          <span>{t("notes.duration")}</span>
          <NumberField
            value={noteDuration}
            min={0.25}
            step={0.25}
            precision={2}
            onchange={(value) => (noteDuration = value)}
          />
        </label>
        <label class="field narrow">
          <span>{t("notes.velocity")}</span>
          <NumberField
            value={noteVelocity}
            min={0}
            max={100}
            step={1}
            precision={0}
            suffix="%"
            onchange={(value) => (noteVelocity = value)}
          />
        </label>
        <button
          class="primary"
          disabled={busy}
          onclick={() => submitNote(owner.id, current.id)}
        >
          {t("notes.add")}
        </button>
      </div>
    {/if}
  {:else}
    <p class="empty">{t("clips.noTrack")}</p>
  {/if}
</section>
