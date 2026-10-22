<script lang="ts">
  import type { GraphConnection, GraphNode, Track } from "../api";
  import { formatNumber, t } from "../i18n.svelte";
  import NumberField from "./NumberField.svelte";

  interface Props {
    track: Track | null;
    busy: boolean;
    onAddGain: (trackId: string) => void;
    onAddAdsr: (trackId: string) => void;
    onAddFilter: (trackId: string) => void;
    onAddMix: (trackId: string) => void;
    onConnect: (
      trackId: string,
      fromNodeId: string,
      fromPort: string,
      toNodeId: string,
      toPort: string,
    ) => void;
    onDisconnect: (
      trackId: string,
      fromNodeId: string,
      fromPort: string,
      toNodeId: string,
      toPort: string,
    ) => void;
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
    onAddFilter,
    onAddMix,
    onConnect,
    onDisconnect,
    onRemove,
    onParamChange,
  }: Props = $props();

  type Position = {
    x: number;
    y: number;
    w: number;
    h: number;
  };

  type Layout = {
    positions: Record<string, Position>;
    width: number;
    height: number;
  };

  const CARD_WIDTH = 220;
  const COLUMN_GAP = 110;
  const HEADER_PORT_Y = 22;

  let canvas = $state<HTMLDivElement>();
  let wireDrag = $state<{ from: string; x: number; y: number } | null>(null);
  let wireHover = $state<{ node: string; port: string } | null>(null);

  let graph = $derived(track?.graph ?? null);
  let nodes = $derived(graph ? Object.values(graph.nodes) : ([] as GraphNode[]));
  let connections = $derived(graph?.connections ?? ([] as GraphConnection[]));
  let hasAdsr = $derived(nodes.some((node) => node.kind === "amp.adsr"));
  let layout = $derived.by(() => buildLayout(nodes, connections));

  function nodeHeight(kind: string): number {
    if (kind === "amp.adsr") return 184;
    if (kind === "filter.lowpass") return 132;
    if (kind === "gain") return 104;
    if (kind === "mix") return 104;
    if (kind === "output") return 80;
    return 90;
  }

  function buildLayout(
    nodeList: GraphNode[],
    edges: GraphConnection[],
  ): Layout {
    if (nodeList.length === 0) {
      return { positions: {}, width: 0, height: 0 };
    }
    const byId = new Map(nodeList.map((node) => [node.id, node]));
    const memo = new Map<string, number>();
    const visiting = new Set<string>();
    function depthOf(id: string): number {
      const cached = memo.get(id);
      if (cached !== undefined) return cached;
      if (visiting.has(id)) return 0;
      visiting.add(id);
      let depth = 0;
      for (const edge of edges) {
        if (edge.to.node === id && byId.has(edge.from.node)) {
          depth = Math.max(depth, depthOf(edge.from.node) + 1);
        }
      }
      visiting.delete(id);
      memo.set(id, depth);
      return depth;
    }
    const groups = new Map<number, GraphNode[]>();
    for (const node of nodeList) {
      const depth = depthOf(node.id);
      const group = groups.get(depth) ?? [];
      group.push(node);
      groups.set(depth, group);
    }
    const positions: Record<string, Position> = {};
    let maxDepth = 0;
    for (const [depth, group] of groups) {
      maxDepth = Math.max(maxDepth, depth);
      group.sort((left, right) => left.id.localeCompare(right.id));
      let y = 30;
      for (const node of group) {
        const h = nodeHeight(node.kind);
        positions[node.id] = {
          x: 30 + depth * (CARD_WIDTH + COLUMN_GAP),
          y,
          w: CARD_WIDTH,
          h,
        };
        y += h + 32;
      }
    }
    let height = 0;
    for (const position of Object.values(positions)) {
      height = Math.max(height, position.y + position.h + 30);
    }
    return {
      positions,
      width: 30 + (maxDepth + 1) * (CARD_WIDTH + COLUMN_GAP),
      height: Math.max(height, 220),
    };
  }

  function nodeLabel(node: GraphNode): string {
    if (node.kind === "instrument.sine") return t("graph.sine");
    if (node.kind === "instrument.silence") return t("graph.silence");
    if (node.kind === "amp.adsr") return t("graph.adsr");
    if (node.kind === "filter.lowpass") return t("graph.filter");
    if (node.kind === "gain") return t("graph.gain");
    if (node.kind === "mix") return t("graph.mix");
    if (node.kind === "output") return t("graph.output");
    return node.kind.toUpperCase();
  }

  function isDevice(node: GraphNode): boolean {
    return (
      node.kind === "amp.adsr" ||
      node.kind === "filter.lowpass" ||
      node.kind === "gain" ||
      node.kind === "mix"
    );
  }

  function paramOf(node: GraphNode, name: string, fallback: number): number {
    return node.params[name] ?? fallback;
  }

  type InputPort = { port: string; y: number };

  function inputPorts(node: GraphNode): InputPort[] {
    if (node.kind.startsWith("instrument.")) return [];
    if (node.kind === "mix") {
      return [
        { port: "in1", y: 22 },
        { port: "in2", y: 54 },
      ];
    }
    return [{ port: "in", y: HEADER_PORT_Y }];
  }

  function outputPoint(connection: GraphConnection) {
    const position = layout.positions[connection.from.node];
    if (!position) return null;
    return { x: position.x + position.w, y: position.y + HEADER_PORT_Y };
  }

  function inputPoint(connection: GraphConnection) {
    const position = layout.positions[connection.to.node];
    const node = graph?.nodes[connection.to.node];
    if (!position || !node) return null;
    const port = inputPorts(node).find(
      (candidate) => candidate.port === connection.to.port,
    );
    return { x: position.x, y: position.y + (port?.y ?? HEADER_PORT_Y) };
  }

  function wirePath(connection: GraphConnection): string {
    const from = outputPoint(connection);
    const to = inputPoint(connection);
    if (!from || !to) return "";
    const bend = Math.max(42, Math.abs(to.x - from.x) * 0.35);
    return `M ${from.x} ${from.y} C ${from.x + bend} ${from.y}, ${to.x - bend} ${to.y}, ${to.x} ${to.y}`;
  }

  function pointer(event: MouseEvent) {
    if (!canvas) return null;
    const rect = canvas.getBoundingClientRect();
    return { x: event.clientX - rect.left, y: event.clientY - rect.top };
  }

  function startWire(event: MouseEvent, node: GraphNode) {
    if (event.button !== 0 || busy) return;
    const point = pointer(event);
    if (!point) return;
    event.preventDefault();
    event.stopPropagation();
    wireDrag = { from: node.id, x: point.x, y: point.y };
  }

  function handleMouseMove(event: MouseEvent) {
    if (!wireDrag) return;
    const point = pointer(event);
    if (!point) return;
    wireDrag = { ...wireDrag, x: point.x, y: point.y };
    const element = document.elementFromPoint(event.clientX, event.clientY);
    const target = element?.closest<HTMLElement>("[data-input-node]");
    const node = target?.dataset.inputNode;
    const port = target?.dataset.inputPort;
    wireHover =
      node && port && node !== wireDrag.from ? { node, port } : null;
    event.preventDefault();
  }

  function handleMouseUp() {
    const drag = wireDrag;
    const target = wireHover;
    wireDrag = null;
    wireHover = null;
    if (!drag || !target || !track) return;
    onConnect(track.id, drag.from, "out", target.node, target.port);
  }

  function removeWire(event: Event, connection: GraphConnection) {
    event.stopPropagation();
    if (!track) return;
    onDisconnect(
      track.id,
      connection.from.node,
      connection.from.port,
      connection.to.node,
      connection.to.port,
    );
  }

  function tempWirePath(): string {
    if (!wireDrag) return "";
    const position = layout.positions[wireDrag.from];
    if (!position) return "";
    const fromX = position.x + position.w;
    const fromY = position.y + HEADER_PORT_Y;
    const bend = Math.max(42, Math.abs(wireDrag.x - fromX) * 0.35);
    return `M ${fromX} ${fromY} C ${fromX + bend} ${fromY}, ${wireDrag.x - bend} ${wireDrag.y}, ${wireDrag.x} ${wireDrag.y}`;
  }
</script>

<svelte:window onmousemove={handleMouseMove} onmouseup={handleMouseUp} />

<section class="section graph-section">
  <div class="section-head graph-head">
    <span class="index">08</span>
    <span class="title">{t("graph.title")}</span>
    <span class="rule"></span>
    <span class="meta">
      {track
        ? `${track.id} · ${nodes.length} ${t("graph.nodes")}`
        : t("graph.noTrack")}
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
        onclick={() => onAddFilter(owner.id)}
      >
        {t("graph.addFilter")}
      </button>
      <button
        class="ghost small graph-add"
        disabled={busy}
        onclick={() => onAddMix(owner.id)}
      >
        {t("graph.addMix")}
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
      <div
        class="graph-canvas"
        bind:this={canvas}
        style:width={`${layout.width}px`}
        style:height={`${layout.height}px`}
      >
        <svg
          class="graph-wires"
          width={layout.width}
          height={layout.height}
          aria-hidden="true"
        >
          {#each connections as connection, index (`${connection.from.node}:${connection.from.port}->${connection.to.node}:${connection.to.port}:${index}`)}
            <path
              class="graph-wire"
              role="button"
              tabindex="0"
              aria-label={t("graph.removeWire")}
              d={wirePath(connection)}
              onclick={(event) => removeWire(event, connection)}
              onkeydown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  removeWire(event, connection);
                }
              }}
            />
          {/each}
          {#if wireDrag}
            <path class="graph-wire temp" d={tempWirePath()} />
          {/if}
        </svg>

        {#each nodes as node (node.id)}
          {@const position = layout.positions[node.id]}
          {@const device = isDevice(node)}
          <div
            class="graph-node"
            role="group"
            aria-label={nodeLabel(node)}
            data-node-id={node.id}
            data-node-kind={node.kind}
            class:device
            class:instrument={node.kind.startsWith("instrument.")}
            class:output={node.kind === "output"}
            class:wire-target={wireHover?.node === node.id}
            style:left={`${position?.x ?? 0}px`}
            style:top={`${position?.y ?? 0}px`}
            style:width={`${position?.w ?? CARD_WIDTH}px`}
            style:height={`${position?.h ?? 96}px`}
          >
            <div class="graph-node-head">
              <span class="graph-node-kind">{nodeLabel(node)}</span>
              <span class="graph-node-id">{node.id}</span>
            </div>

            {#if node.kind !== "output"}
              <button
                type="button"
                class="graph-port graph-port-out"
                aria-label={t("graph.startWire")}
                title={t("graph.startWire")}
                data-output-node={node.id}
                onmousedown={(event) => startWire(event, node)}
              ></button>
            {/if}
            {#each inputPorts(node) as input (input.port)}
              <button
                type="button"
                class="graph-port graph-port-in"
                class:active={wireHover?.node === node.id &&
                  wireHover?.port === input.port}
                style:top={`${input.y - 5}px`}
                aria-label={t("graph.inputPort")}
                data-input-node={node.id}
                data-input-port={input.port}
              ></button>
            {/each}

            {#if node.kind === "instrument.sine"}
              <div class="graph-param-row">
                <span>{t("graph.frequency")}</span>
                <strong
                  >{formatNumber(paramOf(node, "frequency", 440), 2)} Hz</strong
                >
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
            {:else if node.kind === "filter.lowpass"}
              <div class="graph-param-grid">
                <label>
                  <span>{t("graph.cutoff")}</span>
                  <NumberField
                    value={paramOf(node, "cutoff", 1200)}
                    min={20}
                    max={20000}
                    step={1}
                    precision={0}
                    suffix="Hz"
                    disabled={busy}
                    onchange={(value) =>
                      onParamChange(owner.id, node.id, "cutoff", value)}
                  />
                </label>
                <label>
                  <span>{t("graph.resonance")}</span>
                  <NumberField
                    value={Math.round(paramOf(node, "resonance", 0.2) * 100)}
                    min={0}
                    max={95}
                    step={1}
                    precision={0}
                    suffix="%"
                    disabled={busy}
                    onchange={(value) =>
                      onParamChange(
                        owner.id,
                        node.id,
                        "resonance",
                        value / 100,
                      )}
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
            {:else if node.kind === "mix"}
              <div class="graph-param-row">
                <span>{t("graph.mixInputs")}</span>
                <strong>in1 + in2</strong>
              </div>
            {:else}
              <div class="graph-param-row">
                <span>{t("graph.terminal")}</span>
              </div>
            {/if}

            {#if device}
              <div class="graph-node-actions">
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
