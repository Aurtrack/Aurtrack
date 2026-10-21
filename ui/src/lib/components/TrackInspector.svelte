<script lang="ts">
  import type { Device, Track } from "../api";
  import { formatNumber, t } from "../i18n.svelte";

  interface Props {
    track: Track | null;
    busy: boolean;
    onAddGain: (trackId: string) => void;
    onAddAdsr: (trackId: string) => void;
    onRemoveDevice: (path: string) => void;
    onGainChange: (trackId: string, deviceId: string, value: number) => void;
    onParamChange: (
      trackId: string,
      deviceId: string,
      name: string,
      value: number,
    ) => void;
  }

  let {
    track,
    busy,
    onAddGain,
    onAddAdsr,
    onRemoveDevice,
    onGainChange,
    onParamChange,
  }: Props = $props();

  function gainOf(device: Device): number {
    return device.params.gain ?? 1;
  }

  function paramOf(device: Device, name: string, fallback: number): number {
    return device.params[name] ?? fallback;
  }

  function sourceLabel(track: Track): string {
    return track.source.kind === "sine"
      ? t("inspector.source.sine", {
          frequency: formatNumber(track.source.frequency, 2),
        })
      : t("inspector.source.silence");
  }

  function kindLabel(kind: string): string {
    if (kind === "gain") return t("device.kind.gain");
    if (kind === "amp.adsr") return t("device.kind.adsr");
    return kind.toUpperCase();
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
        <div class="device-block">
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
                    Number((event.currentTarget as HTMLInputElement).value) /
                      100,
                  )}
              />
              <span class="readout">
                {formatNumber(gainOf(device) * 100)}%
              </span>
            {:else if device.kind === "amp.adsr"}
              <span class="readout">
                A {formatNumber(paramOf(device, "attack", 0.01), 2)} /
                D {formatNumber(paramOf(device, "decay", 0.12), 2)} /
                S {formatNumber(paramOf(device, "sustain", 0.75) * 100)}% /
                R {formatNumber(paramOf(device, "release", 0.18), 2)}
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
          {#if device.kind === "amp.adsr"}
            <div class="adsr-grid">
              <label class="field narrow">
                <span>{t("device.adsr.attack")}</span>
                <input
                  class="text-input"
                  type="number"
                  min="0.001"
                  max="10"
                  step="0.01"
                  value={paramOf(device, "attack", 0.01)}
                  disabled={busy}
                  onchange={(event) =>
                    onParamChange(
                      current.id,
                      device.id,
                      "attack",
                      Number(
                        (event.currentTarget as HTMLInputElement).value,
                      ),
                    )}
                />
              </label>
              <label class="field narrow">
                <span>{t("device.adsr.decay")}</span>
                <input
                  class="text-input"
                  type="number"
                  min="0"
                  max="10"
                  step="0.01"
                  value={paramOf(device, "decay", 0.12)}
                  disabled={busy}
                  onchange={(event) =>
                    onParamChange(
                      current.id,
                      device.id,
                      "decay",
                      Number(
                        (event.currentTarget as HTMLInputElement).value,
                      ),
                    )}
                />
              </label>
              <label class="field narrow">
                <span>{t("device.adsr.sustain")}</span>
                <input
                  class="text-input"
                  type="number"
                  min="0"
                  max="100"
                  step="1"
                  value={Math.round(
                    paramOf(device, "sustain", 0.75) * 100,
                  )}
                  disabled={busy}
                  onchange={(event) =>
                    onParamChange(
                      current.id,
                      device.id,
                      "sustain",
                      Number(
                        (event.currentTarget as HTMLInputElement).value,
                      ) / 100,
                    )}
                />
              </label>
              <label class="field narrow">
                <span>{t("device.adsr.release")}</span>
                <input
                  class="text-input"
                  type="number"
                  min="0.001"
                  max="10"
                  step="0.01"
                  value={paramOf(device, "release", 0.18)}
                  disabled={busy}
                  onchange={(event) =>
                    onParamChange(
                      current.id,
                      device.id,
                      "release",
                      Number(
                        (event.currentTarget as HTMLInputElement).value,
                      ),
                    )}
                />
              </label>
            </div>
          {/if}
        </div>
      {/each}
      <div class="device-add-row">
        <button
          class="ghost small"
          disabled={busy}
          onclick={() => onAddGain(current.id)}
        >
          {t("inspector.addGain")}
        </button>
        <button
          class="ghost small"
          disabled={busy ||
            current.devices.some((device) => device.kind === "amp.adsr")}
          onclick={() => onAddAdsr(current.id)}
        >
          {t("inspector.addAdsr")}
        </button>
      </div>
    </div>
  {:else}
    <p class="empty">{t("inspector.empty")}</p>
  {/if}
</section>
