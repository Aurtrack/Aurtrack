<script lang="ts">
  import type { GraphNode, Track } from "../api";
  import { formatNumber, t } from "../i18n.svelte";
  import NumberField from "./NumberField.svelte";

  interface Props {
    track: Track | null;
    busy: boolean;
    onAddGain: (trackId: string) => void;
    onAddAdsr: (trackId: string) => void;
    onMove: (trackId: string, nodeId: string, beforeNodeId: string) => void;
    onRemove: (path: string) => void;
    onParamChange: (
      trackId: string,
      nodeId: string,
      name: string,
      value: number,
    ) => void;
  }

  let {
    track,
    busy,
    onAddGain,
    onAddAdsr,
    onMove,
    onRemove,
    onParamChange,
  }: Props = $props();

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

  let hasAdsr = $derived(
    chain.some((node) => node.kind === "amp.adsr"),
  );
  type DragState = {
    id: string;
    startX: number;
    startY: number;
    active: boolean;
  };

  let dragState = $state<DragState | null>(null);
  let dropTarget = $state<string | null>(null);

  function isDeviceNode(node: GraphNode): boolean {
    return node.kind === "amp.adsr" || node.kind === "gain";
  }

  function handleMouseDown(event: MouseEvent, node: GraphNode) {
    if (event.button !== 0 || !isDeviceNode(node) || busy) return;
    event.preventDefault();
    dragState = {
      id: node.id,
      startX: event.clientX,
      startY: event.clientY,
      active: false,
    };
  }

  function handleMouseMove(event: MouseEvent) {
    const state = dragState;
    if (!state) return;
    const distance = Math.hypot(
      event.clientX - state.startX,
      event.clientY - state.startY,
    );
    if (!state.active && distance < 5) return;
    state.active = true;
    const element = document.elementFromPoint(event.clientX, event.clientY);
    const card = element?.closest<HTMLElement>(".graph-node");
    const id = card?.dataset.nodeId;
    const kind = card?.dataset.nodeKind;
    dropTarget =
      id && id !== state.id && kind && !kind.startsWith("instrument.")
        ? id
        : null;
    event.preventDefault();
  }

  function handleMouseUp() {
    const state = dragState;
    const target = dropTarget;
    dragState = null;
    dropTarget = null;
    if (!state?.active || !target || !track) return;
    onMove(track.id, state.id, target);
  }

  function nodeLabel(node: GraphNode): string {
    if (node.kind === "instrument.sine") return t("graph.sine");
    if (node.kind === "instrument.silence") return t("graph.silence");
    if (node.kind === "amp.adsr") return t("graph.adsr");
    if (node.kind === "gain") return t("graph.gain");
    if (node.kind === "output") return t("graph.output");
    return node.kind.toUpperCase();
  }

  function paramOf(node: GraphNode, name: string, fallback: number): number {
    return node.params[name] ?? fallback;
  }

  function moveTarget(deviceId: string, offset: number): GraphNode | null {
    const index = chain.findIndex((node) => node.id === deviceId);
    if (index < 0) return null;
    const target = chain[index + offset];
    if (!target || target.kind.startsWith("instrument.")) return null;
    return target;
  }
</script>

<svelte:window
  onmousemove={handleMouseMove}
  onmouseup={handleMouseUp}
/>

<section class="section graph-section">
  <div class="section-head graph-head">
    <span class="index">08</span>
    <span class="title">{t("graph.title")}</span>
    <span class="rule"></span>
    <span class="meta">
      {track ? `${track.id} · ${chain.length} ${t("graph.nodes")}` : t("graph.noTrack")}
    </span>
  </div>

  {#if track}
    {@const owner = track}
    <div class="graph-toolbar">
      <span class="graph-hint">{t("graph.hint")}</span>
      <span class="rule"></span>
      <button
        class="ghost small graph-add"
        disabled={busy || hasAdsr}
        onclick={() => onAddAdsr(owner.id)}
      >
        {t("graph.addAdsr")}
      </button>
      <button
        class="ghost small graph-add"
        disabled={busy}
        onclick={() => onAddGain(owner.id)}
      >
        {t("graph.addGain")}
      </button>
    </div>

    <div class="graph-board">
      <div class="graph-flow">
        {#each chain as node, index (node.id)}
          {@const isDevice = node.kind === "amp.adsr" || node.kind === "gain"}
          <div
            class="graph-node"
            role="group"
            aria-label={nodeLabel(node)}
            data-node-id={node.id}
            data-node-kind={node.kind}
            class:device={isDevice}
            class:instrument={node.kind.startsWith("instrument.")}
            class:output={node.kind === "output"}
            class:drop-target={dropTarget === node.id}
          >
            <button
              type="button"
              class="graph-node-head"
              aria-label={nodeLabel(node)}
              onmousedown={(event) => handleMouseDown(event, node)}
            >
              {#if isDevice}
                <span class="graph-drag">⠿</span>
              {/if}
              <span class="graph-node-kind">{nodeLabel(node)}</span>
              <span class="graph-node-id">{node.id}</span>
            </button>

            {#if node.kind === "instrument.sine"}
              <div class="graph-param-row">
                <span>{t("graph.frequency")}</span>
                <strong>{formatNumber(paramOf(node, "frequency", 440), 2)} Hz</strong>
              </div>
            {:else if node.kind === "instrument.silence"}
              <div class="graph-param-row">
                <span>{t("graph.silenceSource")}</span>
                <strong>0</strong>
              </div>
            {:else if node.kind === "amp.adsr"}
              <div class="graph-param-grid">
                <label>
                  <span>{t("device.adsr.attack")}</span>
                  <NumberField
                    value={paramOf(node, "attack", 0.01)}
                    min={0.001}
                    max={10}
                    step={0.01}
                    precision={2}
                    disabled={busy}
                    onchange={(value) =>
                      onParamChange(owner.id, node.id, "attack", value)}
                  />
                </label>
                <label>
                  <span>{t("device.adsr.decay")}</span>
                  <NumberField
                    value={paramOf(node, "decay", 0.12)}
                    min={0}
                    max={10}
                    step={0.01}
                    precision={2}
                    disabled={busy}
                    onchange={(value) =>
                      onParamChange(owner.id, node.id, "decay", value)}
                  />
                </label>
                <label>
                  <span>{t("device.adsr.sustain")}</span>
                  <NumberField
                    value={Math.round(paramOf(node, "sustain", 0.75) * 100)}
                    min={0}
                    max={100}
                    step={1}
                    precision={0}
                    suffix="%"
                    disabled={busy}
                    onchange={(value) =>
                      onParamChange(owner.id, node.id, "sustain", value / 100)}
                  />
                </label>
                <label>
                  <span>{t("device.adsr.release")}</span>
                  <NumberField
                    value={paramOf(node, "release", 0.18)}
                    min={0.001}
                    max={10}
                    step={0.01}
                    precision={2}
                    disabled={busy}
                    onchange={(value) =>
                      onParamChange(owner.id, node.id, "release", value)}
                  />
                </label>
              </div>
            {:else if node.kind === "gain"}
              <label class="graph-param-row">
                <span>{t("graph.gain")}</span>
                <NumberField
                  value={paramOf(node, "gain", 1)}
                  min={0}
                  max={4}
                  step={0.05}
                  precision={2}
                  disabled={busy}
                  onchange={(value) =>
                    onParamChange(owner.id, node.id, "gain", value)}
                />
              </label>
            {:else}
              <div class="graph-param-row">
                <span>{t("graph.terminal")}</span>
              </div>
            {/if}

            {#if isDevice}
              <div class="graph-node-actions">
                <button
                  class="mini-button"
                  disabled={busy || !moveTarget(node.id, -1)}
                  onclick={() => {
                    const target = moveTarget(node.id, -1);
                    if (target) onMove(owner.id, node.id, target.id);
                  }}
                >
                  ←
                </button>
                <button
                  class="mini-button"
                  disabled={busy || !moveTarget(node.id, 2)}
                  onclick={() => {
                    const target = moveTarget(node.id, 2);
                    if (target) onMove(owner.id, node.id, target.id);
                  }}
                >
                  →
                </button>
                <button
                  class="mini-button danger"
                  disabled={busy}
                  onclick={() =>
                    onRemove(`/tracks/${owner.id}/nodes/${node.id}`)}
                >
                  ×
                </button>
              </div>
            {/if}
          </div>
          {#if index < chain.length - 1}
            <div class="graph-edge"><span></span></div>
          {/if}
        {/each}
      </div>
    </div>

    <div class="section-foot">
      <span>{t("graph.foot")}</span>
      <span>{t("graph.direction")}</span>
    </div>
  {:else}
    <p class="empty">{t("graph.noTrack")}</p>
  {/if}
</section>
