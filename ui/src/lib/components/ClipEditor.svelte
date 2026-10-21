<script lang="ts">
  import type { Track } from "../api";
  import { formatNumber, t } from "../i18n.svelte";

  interface Props {
    track: Track | null;
    busy: boolean;
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
    selectedNoteId: string;
    onSelectNote: (noteId: string) => void;
    onDuplicateClip: (path: string) => void;
    onDuplicateNote: (path: string) => void;
    onQuantizeNote: (path: string, grid: number) => void;
  }

  let {
    track,
    busy,
    selectedClipId,
    onSelectClip,
    onAddClip,
    onRemoveClip,
    onAddNote,
    onRemoveNote,
    selectedNoteId,
    onSelectNote,
    onDuplicateClip,
    onDuplicateNote,
    onQuantizeNote,
  }: Props = $props();

  let clipName = $state("");
  let clipStart = $state(0);
  let clipDuration = $state(1);
  let notePitch = $state(60);
  let noteStart = $state(0);
  let noteDuration = $state(0.5);
  let noteVelocity = $state(80);

  let clips = $derived(track?.clips ?? []);
  let selectedClip = $derived(
    clips.find((clip) => clip.id === selectedClipId) ?? clips[0] ?? null,
  );

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

  function submitClip(trackId: string) {
    const trimmed = clipName.trim();
    onAddClip(trackId, {
      name:
        trimmed.length > 0
          ? trimmed
          : t("clips.defaultName", { index: clips.length + 1 }),
      start: Number(clipStart),
      duration: Number(clipDuration),
    });
    clipName = "";
  }

  function submitNote(trackId: string, clipId: string) {
    onAddNote(`/tracks/${trackId}/clips/${clipId}`, {
      pitch: Number(notePitch),
      start: Number(noteStart),
      duration: Number(noteDuration),
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
        <input
          class="text-input"
          type="number"
          min="0"
          max="5"
          step="0.25"
          bind:value={clipStart}
        />
      </label>
      <label class="field narrow">
        <span>{t("clips.duration")}</span>
        <input
          class="text-input"
          type="number"
          min="0.25"
          max="5"
          step="0.25"
          bind:value={clipDuration}
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
                start: formatNumber(clip.start, 2),
                end: formatNumber(clip.start + clip.duration, 2),
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
      {@const selectedNote =
        current.notes.find((note) => note.id === selectedNoteId) ?? null}
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
          <div class="note-row" class:selected={note.id === selectedNoteId}>
            <button
              class="note-select"
              disabled={busy}
              onclick={() => onSelectNote(note.id)}
            >
              <span class="device-id">{note.id}</span>
              <span class="note-pitch">{noteName(note.pitch)}</span>
              <span class="track-meta">
                {t("notes.meta", {
                  start: formatNumber(note.start, 2),
                  duration: formatNumber(note.duration, 2),
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

      {#if selectedNote}
        <div class="note-actions">
          <button
            class="mini-button"
            disabled={busy}
            onclick={() =>
              onDuplicateNote(
                `/tracks/${owner.id}/clips/${current.id}/notes/${selectedNote.id}`,
              )}
          >
            {t("notes.duplicate")}
          </button>
          <button
            class="mini-button"
            disabled={busy}
            onclick={() =>
              onQuantizeNote(
                `/tracks/${owner.id}/clips/${current.id}/notes/${selectedNote.id}`,
                0.125,
              )}
          >
            {t("notes.quantize16")}
          </button>
        </div>
      {/if}

      <div class="note-form">
        <label class="field narrow">
          <span>{t("notes.pitch")}</span>
          <input
            class="text-input"
            type="number"
            min="0"
            max="127"
            bind:value={notePitch}
          />
        </label>
        <label class="field narrow">
          <span>{t("notes.start")}</span>
          <input
            class="text-input"
            type="number"
            min="0"
            step="0.125"
            bind:value={noteStart}
          />
        </label>
        <label class="field narrow">
          <span>{t("notes.duration")}</span>
          <input
            class="text-input"
            type="number"
            min="0.0625"
            step="0.125"
            bind:value={noteDuration}
          />
        </label>
        <label class="field narrow">
          <span>{t("notes.velocity")}</span>
          <input
            class="text-input"
            type="number"
            min="0"
            max="100"
            step="1"
            bind:value={noteVelocity}
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
