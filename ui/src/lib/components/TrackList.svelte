<script lang="ts">
  import type { Track } from "../api";
  import { formatNumber, t } from "../i18n.svelte";
  import NumberField from "./NumberField.svelte";

  interface Props {
    tracks: Track[];
    selectedId: string;
    busy: boolean;
    onSelect: (id: string) => void;
    onRemove: (id: string) => void;
    onAdd: (input: {
      name: string;
      source: "sine" | "silence" | "piano";
      frequency: number;
    }) => void;
  }

  let { tracks, selectedId, busy, onSelect, onRemove, onAdd }: Props = $props();

  let name = $state("");
  let source = $state<"sine" | "silence" | "piano">("silence");
  let frequency = $state(220);

  function submit() {
    const trimmed = name.trim();
    onAdd({
      name:
        trimmed.length > 0
          ? trimmed
          : t("tracks.defaultName", { index: tracks.length + 1 }),
      source,
      frequency: Number(frequency),
    });
    name = "";
  }

  function handleSource(event: Event) {
    const value = (event.currentTarget as HTMLSelectElement).value;
    source =
      value === "silence" ? "silence" : value === "piano" ? "piano" : "sine";
  }

  function sourceLabel(track: Track): string {
    if (track.source.kind === "sine") {
      return t("tracks.meta.sine", {
        frequency: formatNumber(track.source.frequency, 1),
        devices: track.devices.length,
      });
    }
    if (track.source.kind === "piano") {
      return t("tracks.meta.piano", { devices: track.devices.length });
    }
    return t("tracks.meta.silence", { devices: track.devices.length });
  }
</script>

<section class="section">
  <div class="section-head">
    <span class="index">02</span>
    <span class="title">{t("tracks.title")}</span>
    <span class="rule"></span>
    <span class="meta">{t("tracks.total", { count: tracks.length })}</span>
  </div>

  <div class="track-form">
    <input
      class="text-input"
      placeholder={t("tracks.namePlaceholder")}
      bind:value={name}
    />
    <select class="text-input" value={source} onchange={handleSource}>
      <option value="silence">{t("tracks.source.silence")}</option>
      <option value="sine">{t("tracks.source.sine")}</option>
      <option value="piano">{t("tracks.source.piano")}</option>
    </select>
    <div class="track-frequency">
      <NumberField
        value={frequency}
        min={20}
        max={20000}
        step={1}
        precision={0}
        disabled={source !== "sine"}
        onchange={(value) => (frequency = value)}
      />
    </div>
    <button class="primary" disabled={busy} onclick={submit}>
      {t("tracks.add")}
    </button>
  </div>
  <p class="form-hint">{t("tracks.sourceHint")}</p>

  <div class="track-list">
    {#if tracks.length === 0}
      <p class="empty">{t("tracks.empty")}</p>
    {/if}
    {#each tracks as track (track.id)}
      <div class="track-row" class:selected={track.id === selectedId}>
        <button class="track-select" onclick={() => onSelect(track.id)}>
          <span class="track-id">{track.id}</span>
          <span class="track-name">{track.name}</span>
          <span class="track-meta">{sourceLabel(track)}</span>
        </button>
        <button
          class="track-remove"
          title={t("tracks.removeTitle")}
          disabled={busy}
          onclick={() => onRemove(track.id)}
        >
          ×
        </button>
      </div>
    {/each}
  </div>
</section>
