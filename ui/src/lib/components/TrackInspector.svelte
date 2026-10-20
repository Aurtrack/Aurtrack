<script lang="ts">
  import type { Device, Track } from "../api";
  import { formatNumber, t } from "../i18n.svelte";

  interface Props {
    track: Track | null;
    busy: boolean;
    onAddGain: (trackId: string) => void;
    onRemoveDevice: (path: string) => void;
    onGainChange: (trackId: string, deviceId: string, value: number) => void;
  }

  let { track, busy, onAddGain, onRemoveDevice, onGainChange }: Props =
    $props();

  function gainOf(device: Device): number {
    return device.params.gain ?? 1;
  }

  function sourceLabel(track: Track): string {
    return track.source.kind === "sine"
      ? t("inspector.source.sine", {
          frequency: formatNumber(track.source.frequency, 2),
        })
      : t("inspector.source.silence");
  }

  function kindLabel(kind: string): string {
    return kind === "gain" ? t("device.kind.gain") : kind.toUpperCase();
  }
</script>

<section class="section">
  <div class="section-head">
    <span class="index">04</span>
    <span class="title">{t("inspector.title")}</span>
    <span class="rule"></span>
    <span class="meta">{track ? track.id : t("inspector.none")}</span>
  </div>

  {#if track}
    {@const current = track}
    <div class="inspector-head">
      <span class="track-name">{current.name}</span>
      <span class="track-meta">{sourceLabel(current)}</span>
    </div>
    <div class="device-list">
      {#each current.devices as device (device.id)}
        <div class="device-row">
          <span class="device-id">{device.id}</span>
          <span class="device-kind">{kindLabel(device.kind)}</span>
          {#if device.kind === "gain"}
            <input
              type="range"
              min="0"
              max="200"
              value={Math.round(gainOf(device) * 100)}
              onchange={(event) =>
                onGainChange(
                  current.id,
                  device.id,
                  Number((event.currentTarget as HTMLInputElement).value) / 100,
                )}
            />
            <span class="readout">
              {formatNumber(gainOf(device) * 100)}%
            </span>
          {/if}
          <button
            class="track-remove"
            title={t("inspector.removeDeviceTitle")}
            disabled={busy}
            onclick={() =>
              onRemoveDevice(`/tracks/${current.id}/devices/${device.id}`)}
          >
            ×
          </button>
        </div>
      {/each}
      <button
        class="ghost small"
        disabled={busy}
        onclick={() => onAddGain(current.id)}
      >
        {t("inspector.addGain")}
      </button>
    </div>
  {:else}
    <p class="empty">{t("inspector.empty")}</p>
  {/if}
</section>
