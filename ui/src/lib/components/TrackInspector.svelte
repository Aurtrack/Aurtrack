<script lang="ts">
  import type { Device, Track } from "../api";

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
      ? `SINE ${track.source.frequency.toFixed(2)} HZ`
      : "SILENCE";
  }
</script>

<section class="section">
  <div class="section-head">
    <span class="index">03</span>
    <span class="title">INSPECTOR</span>
    <span class="rule"></span>
    <span class="meta">{track ? track.id : "NONE"}</span>
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
          <span class="device-kind">{device.kind.toUpperCase()}</span>
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
              {Math.round(gainOf(device) * 100)}%
            </span>
          {/if}
          <button
            class="track-remove"
            title="删除设备"
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
        + GAIN
      </button>
    </div>
  {:else}
    <p class="empty">NO TRACK SELECTED</p>
  {/if}
</section>
