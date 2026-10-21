<script lang="ts">
  import type { Device, GraphNode, Track } from "../api";
  import { formatNumber, t } from "../i18n.svelte";
  import NumberField from "./NumberField.svelte";

  interface Props {
    track: Track | null;
    busy: boolean;
    onAddGain: (trackId: string) => void;
    onAddAdsr: (trackId: string) => void;
    onMoveDevice: (
      trackId: string,
      nodeId: string,
      beforeNodeId: string,
    ) => void;
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
    onMoveDevice,
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

  let chain = $derived.by(() => {
    const graph = track?.graph;
    if (!graph) return [] as GraphNode[];
    const start = Object.values(graph.nodes).find((node) =>
      node.kind.startsWith("instrument."),
    );
    if (!start) return [] as GraphNode[];
    const result: GraphNode[] = [];
    const visited = new Set<string>();
    let current: GraphNode | undefined = start;
    while (current && !visited.has(current.id) && result.length < 32) {
      result.push(current);
      visited.add(current.id);
      const connection = graph.connections.find(
        (item) => item.from.node === current?.id && item.from.port === "out",
      );
      current = connection ? graph.nodes[connection.to.node] : undefined;
    }
    return result;
  });

  function moveTarget(deviceId: string, offset: number): GraphNode | null {
    const index = chain.findIndex((node) => node.id === deviceId);
    if (index < 0) return null;
    const target = chain[index + offset];
    if (!target || target.kind.startsWith("instrument.")) return null;
    return target;
  }

  function nodeLabel(node: GraphNode): string {
    if (node.kind === "instrument.sine") return "SINE";
    if (node.kind === "instrument.silence") return "SILENCE";
    if (node.kind === "amp.adsr") return "ADSR";
    if (node.kind === "gain") return "GAIN";
    if (node.kind === "output") return "OUT";
    return node.kind.toUpperCase();
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
    {#if chain.length > 0}
      <div class="node-chain" title={t("inspector.chain")}>
        {#each chain as node, index (node.id)}
          <span class="node-chip" title={node.id}>{nodeLabel(node)}</span>
          {#if index < chain.length - 1}
            <span class="node-arrow">›</span>
          {/if}
        {/each}
      </div>
    {/if}
    <div class="device-list">
      {#each current.devices as device (device.id)}
        <div class="device-block">
          <div class="device-row">
            <span class="device-id">{device.id}</span>
            <span class="device-kind">{kindLabel(device.kind)}</span>
            {#if device.kind === "gain"}
              <NumberField
                value={gainOf(device)}
                min={0}
                max={4}
                step={0.05}
                precision={2}
                disabled={busy}
                onchange={(value) =>
                  onGainChange(current.id, device.id, value)}
              />
            {:else if device.kind === "amp.adsr"}
              <span class="readout">
                A {formatNumber(paramOf(device, "attack", 0.01), 2)} /
                D {formatNumber(paramOf(device, "decay", 0.12), 2)} /
                S {formatNumber(paramOf(device, "sustain", 0.75) * 100)}% /
                R {formatNumber(paramOf(device, "release", 0.18), 2)}
              </span>
            {/if}
            <button
              class="node-move"
              title={t("inspector.moveUp")}
              disabled={busy || !moveTarget(device.id, -1)}
              onclick={() => {
                const up = moveTarget(device.id, -1);
                if (up) onMoveDevice(current.id, device.id, up.id);
              }}
            >
              ↑
            </button>
            <button
              class="node-move"
              title={t("inspector.moveDown")}
              disabled={busy || !moveTarget(device.id, 2)}
              onclick={() => {
                const down = moveTarget(device.id, 2);
                if (down) onMoveDevice(current.id, device.id, down.id);
              }}
            >
              ↓
            </button>
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
                <NumberField
                  value={paramOf(device, "attack", 0.01)}
                  min={0.001}
                  max={10}
                  step={0.01}
                  precision={2}
                  disabled={busy}
                  onchange={(value) =>
                    onParamChange(current.id, device.id, "attack", value)}
                />
              </label>
              <label class="field narrow">
                <span>{t("device.adsr.decay")}</span>
                <NumberField
                  value={paramOf(device, "decay", 0.12)}
                  min={0}
                  max={10}
                  step={0.01}
                  precision={2}
                  disabled={busy}
                  onchange={(value) =>
                    onParamChange(current.id, device.id, "decay", value)}
                />
              </label>
              <label class="field narrow">
                <span>{t("device.adsr.sustain")}</span>
                <NumberField
                  value={Math.round(paramOf(device, "sustain", 0.75) * 100)}
                  min={0}
                  max={100}
                  step={1}
                  precision={0}
                  suffix="%"
                  disabled={busy}
                  onchange={(value) =>
                    onParamChange(
                      current.id,
                      device.id,
                      "sustain",
                      value / 100,
                    )}
                />
              </label>
              <label class="field narrow">
                <span>{t("device.adsr.release")}</span>
                <NumberField
                  value={paramOf(device, "release", 0.18)}
                  min={0.001}
                  max={10}
                  step={0.01}
                  precision={2}
                  disabled={busy}
                  onchange={(value) =>
                    onParamChange(current.id, device.id, "release", value)}
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
