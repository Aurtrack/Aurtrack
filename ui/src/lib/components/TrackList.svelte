<script lang="ts">
  import type { Track } from "../api";

  interface Props {
    tracks: Track[];
    selectedId: string;
    busy: boolean;
    onSelect: (id: string) => void;
    onRemove: (id: string) => void;
    onAdd: (input: {
      name: string;
      source: "sine" | "silence";
      frequency: number;
    }) => void;
  }

  let { tracks, selectedId, busy, onSelect, onRemove, onAdd }: Props = $props();

  let name = $state("");
  let source = $state<"sine" | "silence">("sine");
  let frequency = $state(220);

  function submit() {
    const trimmed = name.trim();
    onAdd({
      name: trimmed.length > 0 ? trimmed : `Track ${tracks.length + 1}`,
      source,
      frequency: Number(frequency),
    });
    name = "";
  }

  function handleSource(event: Event) {
    const value = (event.currentTarget as HTMLSelectElement).value;
    source = value === "silence" ? "silence" : "sine";
  }

  function sourceLabel(track: Track): string {
    return track.source.kind === "sine"
      ? `SINE ${track.source.frequency.toFixed(1)} HZ`
      : "SILENCE";
  }
</script>

<section class="section">
  <div class="section-head">
    <span class="index">02</span>
    <span class="title">TRACKS</span>
    <span class="rule"></span>
    <span class="meta">{tracks.length} TOTAL</span>
  </div>

  <div class="track-form">
    <input class="text-input" placeholder="TRACK NAME" bind:value={name} />
    <select class="text-input" value={source} onchange={handleSource}>
      <option value="sine">SINE</option>
      <option value="silence">SILENCE</option>
    </select>
    <input
      class="text-input number"
      type="number"
      min="20"
      max="20000"
      bind:value={frequency}
      disabled={source === "silence"}
    />
    <button class="primary" disabled={busy} onclick={submit}>ADD</button>
  </div>

  <div class="track-list">
    {#if tracks.length === 0}
      <p class="empty">NO TRACKS</p>
    {/if}
    {#each tracks as track (track.id)}
      <div class="track-row" class:selected={track.id === selectedId}>
        <button class="track-select" onclick={() => onSelect(track.id)}>
          <span class="track-id">{track.id}</span>
          <span class="track-name">{track.name}</span>
          <span class="track-meta">
            {sourceLabel(track)} · {track.devices.length} DEV
          </span>
        </button>
        <button
          class="track-remove"
          title="删除轨道"
          disabled={busy}
          onclick={() => onRemove(track.id)}
        >
          ×
        </button>
      </div>
    {/each}
  </div>
</section>
