<script lang="ts">
  import type { GraphConnection, GraphNode, Track } from "../api";
  import { formatNumber, t } from "../i18n.svelte";
  import NumberField from "./NumberField.svelte";

  interface Props {
    track: Track | null;
    busy: boolean;
    onAddNode: (
      trackId: string,
      kind: string,
      x: number,
      y: number,
    ) => void | Promise<string | undefined>;
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
    onPlace: (
      trackId: string,
      nodes: { id: string; x: number; y: number }[],
    ) => void | Promise<void>;
    onDuplicate: (
      paths: string[],
    ) => Promise<{ source: string; node_id: string }[] | undefined> | void;
    onRemove: (path: string) => void;
    onRemoveMany: (paths: string[]) => void;
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
    onAddNode,
    onConnect,
    onDisconnect,
    onPlace,
    onDuplicate,
    onRemove,
    onRemoveMany,
    onParamChange,
  }: Props = $props();

  type Point = { x: number; y: number };
  type Position = Point & { w: number; h: number };
  type Layout = {
    positions: Record<string, Position>;
    width: number;
    height: number;
  };
  type WireDrag = {
    side: "out" | "in";
    node: string;
    port: string;
    x: number;
    y: number;
  };
  type NodeDrag = {
    start: Point;
    ids: string[];
    origins: Record<string, Point>;
    moved: boolean;
  };
  type PanDrag = {
    startX: number;
    startY: number;
    viewX: number;
    viewY: number;
  };
  type Marquee = {
    x1: number;
    y1: number;
    x2: number;
    y2: number;
    additive: boolean;
  };
  type AddMenu = { x: number; y: number };
  type NodeMenu = { x: number; y: number; nodeId: string };

  const CARD_WIDTH = 220;
  const COLUMN_GAP = 110;
  const HEADER_PORT_Y = 22;
  const MIN_SCALE = 0.25;
  const MAX_SCALE = 2.5;

  let board = $state<HTMLDivElement>();
  let view = $state({ x: 20, y: 30, scale: 0.8 });
  let selected = $state<Set<string>>(new Set());
  let draft = $state<Record<string, Point>>({});
  let nodeDrag = $state<NodeDrag | null>(null);
  let panDrag = $state<PanDrag | null>(null);
  let marquee = $state<Marquee | null>(null);
  let wireDrag = $state<WireDrag | null>(null);
  let wireHover = $state<{ node: string; port: string } | null>(null);
  let addMenu = $state<AddMenu | null>(null);
  let nodeMenu = $state<NodeMenu | null>(null);
  let spaceHeld = $state(false);
  let lastPointer = $state<Point>({ x: 0, y: 0 });

  let graph = $derived(track?.graph ?? null);
  let nodes = $derived(graph ? Object.values(graph.nodes) : ([] as GraphNode[]));
  let connections = $derived(graph?.connections ?? ([] as GraphConnection[]));
  let hasAdsr = $derived(nodes.some((node) => node.kind === "amp.adsr"));
  let autoLayout = $derived.by(() => buildLayout(nodes, connections));

  let nodePositions = $derived.by(() => {
    const result: Record<string, Position> = {};
    for (const node of nodes) {
      const size = nodeSize(node);
      const stored = node.position;
      const auto = autoLayout.positions[node.id];
      const base = stored
        ? { x: stored[0], y: stored[1] }
        : (auto ?? { x: 30, y: 30 });
      const point = draft[node.id] ?? base;
      result[node.id] = { ...point, ...size };
    }
    return result;
  });

  let lastTrackId = "";

  $effect(() => {
    const id = track?.id ?? "";
    if (id === lastTrackId) return;
    lastTrackId = id;
    selected = new Set();
    draft = {};
    addMenu = null;
    nodeMenu = null;
    view = { x: 20, y: 30, scale: 0.8 };
  });

  let canvasSize = $derived.by(() => {
    let width = 1600;
    let height = 900;
    for (const position of Object.values(nodePositions)) {
      width = Math.max(width, position.x + position.w + 200);
      height = Math.max(height, position.y + position.h + 200);
    }
    return { width, height };
  });

  function nodeHeight(kind: string): number {
    if (kind === "amp.adsr") return 184;
    if (kind === "filter.lowpass") return 132;
    if (kind === "instrument.piano") return 184;
    if (kind === "gain") return 104;
    if (kind === "mix") return 104;
    if (kind === "plugin") return 120;
    if (kind === "output") return 80;
    return 90;
  }

  function nodeSize(node: GraphNode) {
    return { w: CARD_WIDTH, h: nodeHeight(node.kind) };
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
    if (node.kind === "instrument.piano") return t("graph.piano");
    if (node.kind === "amp.adsr") return t("graph.adsr");
    if (node.kind === "filter.lowpass") return t("graph.filter");
    if (node.kind === "gain") return t("graph.gain");
    if (node.kind === "mix") return t("graph.mix");
    if (node.kind === "plugin") return t("graph.plugin");
    if (node.kind === "output") return t("graph.output");
    return node.kind.toUpperCase();
  }

  function isDevice(node: GraphNode): boolean {
    return (
      node.kind === "amp.adsr" ||
      node.kind === "filter.lowpass" ||
      node.kind === "gain" ||
      node.kind === "mix" ||
      node.kind === "plugin"
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

  function outputPoint(nodeId: string): Point | null {
    const position = nodePositions[nodeId];
    if (!position) return null;
    return { x: position.x + position.w, y: position.y + HEADER_PORT_Y };
  }

  function inputPoint(nodeId: string, portName: string): Point | null {
    const position = nodePositions[nodeId];
    const node = graph?.nodes[nodeId];
    if (!position || !node) return null;
    const port = inputPorts(node).find(
      (candidate) => candidate.port === portName,
    );
    return { x: position.x, y: position.y + (port?.y ?? HEADER_PORT_Y) };
  }

  function wirePath(connection: GraphConnection): string {
    const from = outputPoint(connection.from.node);
    const to = inputPoint(connection.to.node, connection.to.port);
    if (!from || !to) return "";
    return curve(from, to);
  }

  function curve(from: Point, to: Point): string {
    const bend = Math.max(48, Math.abs(to.x - from.x) * 0.4);
    return `M ${from.x} ${from.y} C ${from.x + bend} ${from.y}, ${to.x - bend} ${to.y}, ${to.x} ${to.y}`;
  }

  function pointer(event: { clientX: number; clientY: number }): Point | null {
    if (!board) return null;
    const rect = board.getBoundingClientRect();
    return { x: event.clientX - rect.left, y: event.clientY - rect.top };
  }

  function worldPoint(event: { clientX: number; clientY: number }): Point | null {
    const point = pointer(event);
    if (!point) return null;
    return {
      x: (point.x - view.x) / view.scale,
      y: (point.y - view.y) / view.scale,
    };
  }

  function clampScale(scale: number): number {
    return Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale));
  }

  function handleWheel(event: WheelEvent) {
    if (!board) return;
    event.preventDefault();
    const point = pointer(event);
    if (!point) return;
    const next = clampScale(view.scale * Math.exp(-event.deltaY * 0.0012));
    const worldX = (point.x - view.x) / view.scale;
    const worldY = (point.y - view.y) / view.scale;
    view = {
      x: point.x - worldX * next,
      y: point.y - worldY * next,
      scale: next,
    };
  }

  function frameNodes(group: GraphNode[]) {
    if (!board || group.length === 0) return;
    const rect = board.getBoundingClientRect();
    let minX = Infinity;
    let minY = Infinity;
    let maxX = -Infinity;
    let maxY = -Infinity;
    for (const node of group) {
      const position = nodePositions[node.id];
      if (!position) continue;
      minX = Math.min(minX, position.x);
      minY = Math.min(minY, position.y);
      maxX = Math.max(maxX, position.x + position.w);
      maxY = Math.max(maxY, position.y + position.h);
    }
    if (!Number.isFinite(minX) || !Number.isFinite(minY)) return;
    const width = Math.max(1, maxX - minX);
    const height = Math.max(1, maxY - minY);
    const scale = clampScale(
      Math.min(1, (rect.width - 100) / width, (rect.height - 100) / height),
    );
    view = {
      x: (rect.width - width * scale) / 2 - minX * scale,
      y: (rect.height - height * scale) / 2 - minY * scale,
      scale,
    };
  }

  function frameAll() {
    frameNodes(nodes);
  }

  function frameSelected() {
    const group = nodes.filter((node) => selected.has(node.id));
    frameNodes(group.length > 0 ? group : nodes);
  }

  function boardMouseDown(event: MouseEvent) {
    if (event.button === 1 || (spaceHeld && event.button === 0)) {
      event.preventDefault();
      panDrag = {
        startX: event.clientX,
        startY: event.clientY,
        viewX: view.x,
        viewY: view.y,
      };
      return;
    }
    if (event.button !== 0) return;
    const target = event.target as Element | null;
    if (target?.closest(".graph-node, .graph-wire-hit, .graph-add-menu")) {
      return;
    }
    const point = worldPoint(event);
    if (!point) return;
    const additive = event.shiftKey;
    if (!additive) selected = new Set();
    marquee = {
      x1: point.x,
      y1: point.y,
      x2: point.x,
      y2: point.y,
      additive,
    };
    addMenu = null;
    nodeMenu = null;
    event.preventDefault();
  }

  function selectNode(node: GraphNode, event: MouseEvent) {
    addMenu = null;
    if (event.button !== 0) return;
    if (event.shiftKey) {
      const next = new Set(selected);
      if (next.has(node.id)) next.delete(node.id);
      else next.add(node.id);
      selected = next;
    } else if (!selected.has(node.id)) {
      selected = new Set([node.id]);
    }
  }

  function isInteractive(target: EventTarget | null): boolean {
    const element = target as Element | null;
    return Boolean(
      element?.closest(
        "input, textarea, select, button, [contenteditable='true'], .graph-context-menu",
      ),
    );
  }

  function nodeMouseDown(event: MouseEvent, node: GraphNode) {
    if (event.button !== 0 || busy || spaceHeld) return;
    if (isInteractive(event.target)) return;
    nodeMenu = null;
    event.stopPropagation();
    event.preventDefault();
    selectNode(node, event);
    if (!selected.has(node.id)) return;
    const ids = [...selected].filter((id) => Boolean(graph?.nodes[id]));
    const origins: Record<string, Point> = {};
    for (const id of ids) {
      const position = nodePositions[id];
      if (position) origins[id] = { x: position.x, y: position.y };
    }
    const start = worldPoint(event);
    if (!start) return;
    nodeDrag = { start, ids, origins, moved: false };
    addMenu = null;
  }

  function openNodeMenu(event: MouseEvent, node: GraphNode) {
    if (!isDevice(node)) return;
    event.preventDefault();
    event.stopPropagation();
    if (!selected.has(node.id)) selected = new Set([node.id]);
    addMenu = null;
    nodeMenu = { x: event.clientX, y: event.clientY, nodeId: node.id };
  }

  function deleteFromMenu() {
    if (!nodeMenu) return;
    nodeMenu = null;
    deleteSelected();
  }

  function duplicateFromMenu() {
    if (!nodeMenu) return;
    nodeMenu = null;
    void duplicateSelected();
  }

  function startWire(event: MouseEvent, node: GraphNode, side: "out" | "in", port: string) {
    if (event.button !== 0 || busy || spaceHeld) return;
    event.preventDefault();
    event.stopPropagation();
    const point = worldPoint(event);
    if (!point) return;
    wireDrag = { side, node: node.id, port, x: point.x, y: point.y };
    wireHover = null;
  }

  function updateWireHover(event: MouseEvent) {
    const drag = wireDrag;
    if (!drag) return;
    const selector = drag.side === "out" ? "[data-input-node]" : "[data-output-node]";
    const element = document.elementFromPoint(event.clientX, event.clientY);
    const target = element?.closest<HTMLElement>(selector);
    if (drag.side === "out") {
      const node = target?.dataset.inputNode;
      const port = target?.dataset.inputPort;
      wireHover =
        node && port && node !== drag.node ? { node, port } : null;
    } else {
      const node = target?.dataset.outputNode;
      wireHover =
        node && node !== drag.node ? { node, port: "out" } : null;
    }
  }

  function handleMouseMove(event: MouseEvent) {
    lastPointer = pointer(event) ?? lastPointer;
    if (panDrag) {
      view = {
        ...view,
        x: panDrag.viewX + event.clientX - panDrag.startX,
        y: panDrag.viewY + event.clientY - panDrag.startY,
      };
      return;
    }
    if (nodeDrag) {
      const point = worldPoint(event);
      if (!point) return;
      const dx = point.x - nodeDrag.start.x;
      const dy = point.y - nodeDrag.start.y;
      if (!nodeDrag.moved && Math.hypot(dx, dy) < 2) return;
      nodeDrag.moved = true;
      const next = { ...draft };
      for (const [id, origin] of Object.entries(nodeDrag.origins)) {
        next[id] = {
          x: origin.x + dx,
          y: origin.y + dy,
        };
      }
      draft = next;
      event.preventDefault();
      return;
    }
    if (marquee) {
      const point = worldPoint(event);
      if (!point) return;
      marquee = { ...marquee, x2: point.x, y2: point.y };
      return;
    }
    if (wireDrag) {
      const point = worldPoint(event);
      if (point) wireDrag = { ...wireDrag, x: point.x, y: point.y };
      updateWireHover(event);
    }
  }

  function finishMarquee() {
    const area = marquee;
    marquee = null;
    if (!area) return;
    if (Math.abs(area.x2 - area.x1) < 3 && Math.abs(area.y2 - area.y1) < 3) {
      return;
    }
    const left = Math.min(area.x1, area.x2);
    const right = Math.max(area.x1, area.x2);
    const top = Math.min(area.y1, area.y2);
    const bottom = Math.max(area.y1, area.y2);
    const hits: string[] = [];
    for (const node of nodes) {
      const position = nodePositions[node.id];
      if (!position) continue;
      if (
        position.x < right &&
        position.x + position.w > left &&
        position.y < bottom &&
        position.y + position.h > top
      ) {
        hits.push(node.id);
      }
    }
    selected = area.additive ? new Set([...selected, ...hits]) : new Set(hits);
  }

  function handleMouseUp() {
    if (panDrag) {
      panDrag = null;
      return;
    }
    if (marquee) {
      finishMarquee();
      return;
    }
    if (nodeDrag) {
      const drag = nodeDrag;
      nodeDrag = null;
      if (drag.moved && track) {
        const placed = drag.ids
          .map((id) => {
            const point = draft[id];
            return point ? { id, x: point.x, y: point.y } : null;
          })
          .filter((item): item is { id: string; x: number; y: number } =>
            Boolean(item),
          );
        if (placed.length > 0) {
          Promise.resolve(onPlace(track.id, placed)).finally(() => {
            draft = {};
          });
        }
      }
      return;
    }
    if (wireDrag) {
      const drag = wireDrag;
      const target = wireHover;
      wireDrag = null;
      wireHover = null;
      if (!track || !target) return;
      if (drag.side === "out") {
        onConnect(track.id, drag.node, drag.port, target.node, target.port);
      } else {
        onConnect(track.id, target.node, "out", drag.node, drag.port);
      }
    }
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
    const anchor =
      wireDrag.side === "out"
        ? outputPoint(wireDrag.node)
        : inputPoint(wireDrag.node, wireDrag.port);
    if (!anchor) return "";
    return curve(anchor, { x: wireDrag.x, y: wireDrag.y });
  }

  function marqueeStyle(): string {
    if (!marquee) return "";
    const left = Math.min(marquee.x1, marquee.x2);
    const top = Math.min(marquee.y1, marquee.y2);
    const width = Math.abs(marquee.x2 - marquee.x1);
    const height = Math.abs(marquee.y2 - marquee.y1);
    return `left:${left}px;top:${top}px;width:${width}px;height:${height}px`;
  }

  function selectedPaths(): string[] {
    if (!track) return [];
    return nodes
      .filter((node) => selected.has(node.id) && isDevice(node))
      .map((node) => `/tracks/${track.id}/nodes/${node.id}`);
  }

  function deleteSelected() {
    const paths = selectedPaths();
    if (paths.length === 0) return;
    selected = new Set();
    onRemoveMany(paths);
  }

  async function duplicateSelected() {
    if (!track) return;
    const paths = nodes
      .filter(
        (node) =>
          selected.has(node.id) &&
          isDevice(node) &&
          node.kind !== "amp.adsr",
      )
      .map((node) => `/tracks/${track.id}/nodes/${node.id}`);
    if (paths.length === 0) return;
    const copied = await onDuplicate(paths);
    if (copied && copied.length > 0) {
      selected = new Set(copied.map((item) => item.node_id));
    }
  }

  function isEditing(): boolean {
    const element = document.activeElement;
    return (
      element instanceof HTMLInputElement ||
      element instanceof HTMLTextAreaElement ||
      element instanceof HTMLSelectElement ||
      (element instanceof HTMLElement && element.isContentEditable)
    );
  }

  function handleKeyDown(event: KeyboardEvent) {
    if ((event.code === "Space" || event.key === " ") && !isEditing()) {
      spaceHeld = true;
      event.preventDefault();
      return;
    }
    if (isEditing()) return;
    if (event.key === "Home") {
      event.preventDefault();
      frameAll();
      return;
    }
    if (event.key.toLowerCase() === "f" || event.code === "NumpadDecimal") {
      event.preventDefault();
      frameSelected();
      return;
    }
    if (
      event.shiftKey &&
      !event.ctrlKey &&
      !event.metaKey &&
      event.key.toLowerCase() === "d"
    ) {
      event.preventDefault();
      void duplicateSelected();
      return;
    }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "d") {
      event.preventDefault();
      void duplicateSelected();
      return;
    }
    if (event.key === "Delete" || event.key === "Backspace") {
      event.preventDefault();
      deleteSelected();
      return;
    }
    if (
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey &&
      event.key.toLowerCase() === "x"
    ) {
      event.preventDefault();
      deleteSelected();
      return;
    }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "a") {
      event.preventDefault();
      selected = new Set(nodes.map((node) => node.id));
      return;
    }
    if (event.shiftKey && event.key.toLowerCase() === "a") {
      event.preventDefault();
      addMenu = {
        x: Math.max(10, lastPointer.x),
        y: Math.max(10, lastPointer.y),
      };
      return;
    }
    if (event.key === "Escape") {
      selected = new Set();
      addMenu = null;
      nodeMenu = null;
    }
  }

  function handleKeyUp(event: KeyboardEvent) {
    if (event.code === "Space" || event.key === " ") spaceHeld = false;
  }

  function addNode(kind: string, at?: Point) {
    if (!track) return;
    const point =
      at ??
      (addMenu
        ? { x: addMenu.x, y: addMenu.y }
        : {
            x: (board?.clientWidth ?? 800) / 2,
            y: (board?.clientHeight ?? 500) / 2,
          });
    const world = {
      x: (point.x - view.x) / view.scale,
      y: (point.y - view.y) / view.scale,
    };
    addMenu = null;
    void onAddNode(track.id, kind, world.x, world.y);
  }
</script>

<svelte:window
  onmousemove={handleMouseMove}
  onmouseup={handleMouseUp}
  onkeydown={handleKeyDown}
  onkeyup={handleKeyUp}
/>

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
      <span class="graph-zoom">{Math.round(view.scale * 100)}%</span>
      <button class="ghost small graph-add" onclick={frameAll}>
        {t("graph.frameAll")}
      </button>
      <button
        class="ghost small graph-add"
        disabled={busy || hasAdsr}
        onclick={() => addNode("amp.adsr")}
      >
        {t("graph.addAdsr")}
      </button>
      <button
        class="ghost small graph-add"
        disabled={busy}
        onclick={() => addNode("filter.lowpass")}
      >
        {t("graph.addFilter")}
      </button>
      <button
        class="ghost small graph-add"
        disabled={busy}
        onclick={() => addNode("mix")}
      >
        {t("graph.addMix")}
      </button>
      <button
        class="ghost small graph-add"
        disabled={busy}
        onclick={() => addNode("gain")}
      >
        {t("graph.addGain")}
      </button>
    </div>

    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div
      class="graph-board"
      class:panning={Boolean(panDrag)}
      bind:this={board}
      role="application"
      aria-label={t("graph.title")}
      onmousedown={boardMouseDown}
      onwheel={handleWheel}
      style:background-position={`${view.x}px ${view.y}px`}
      style:background-size={`${24 * view.scale}px ${24 * view.scale}px`}
    >
      <div
        class="graph-canvas"
        style:width={`${canvasSize.width}px`}
        style:height={`${canvasSize.height}px`}
        style:transform={`translate(${view.x}px, ${view.y}px) scale(${view.scale})`}
      >
        <svg
          class="graph-wires"
          width={canvasSize.width}
          height={canvasSize.height}
          aria-hidden="true"
        >
          {#each connections as connection, index (`${connection.from.node}:${connection.from.port}->${connection.to.node}:${connection.to.port}:${index}`)}
            <path
              class="graph-wire-hit"
              d={wirePath(connection)}
              role="button"
              tabindex="0"
              aria-label={t("graph.removeWire")}
              onclick={(event) => removeWire(event, connection)}
              onkeydown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  removeWire(event, connection);
                }
              }}
            />
            <path class="graph-wire" d={wirePath(connection)} />
          {/each}
          {#if wireDrag}
            <path class="graph-wire temp" d={tempWirePath()} />
          {/if}
        </svg>

        {#if marquee}
          <div class="graph-marquee" style={marqueeStyle()}></div>
        {/if}

        {#each nodes as node (node.id)}
          {@const position = nodePositions[node.id]}
          {@const device = isDevice(node)}
          {@const isSelected = selected.has(node.id)}
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
          <div
            class="graph-node"
            class:device
            class:instrument={node.kind.startsWith("instrument.")}
            class:output={node.kind === "output"}
            class:selected={isSelected}
            class:wire-target={wireHover?.node === node.id}
            role="group"
            aria-label={nodeLabel(node)}
            data-node-id={node.id}
            data-node-kind={node.kind}
            style:left={`${position?.x ?? 0}px`}
            style:top={`${position?.y ?? 0}px`}
            style:min-height={`${position?.h ?? 90}px`}
            style:width={`${position?.w ?? CARD_WIDTH}px`}
            onmousedown={(event) => nodeMouseDown(event, node)}
            oncontextmenu={(event) => openNodeMenu(event, node)}
          >
            <div
              class="graph-node-head"
              role="button"
              tabindex="0"
              aria-label={nodeLabel(node)}
            >
              <span class="graph-node-kind">{nodeLabel(node)}</span>
              <span class="graph-node-id">{node.id}</span>
            </div>

            {#if node.kind !== "output"}
              <button
                type="button"
                class="graph-port graph-port-out"
                class:active={wireHover?.node === node.id &&
                  wireDrag?.side === "in"}
                aria-label={t("graph.startWire")}
                title={t("graph.startWire")}
                data-output-node={node.id}
                onmousedown={(event) => startWire(event, node, "out", "out")}
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
                onmousedown={(event) => startWire(event, node, "in", input.port)}
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
            {:else if node.kind === "instrument.piano"}
              <div class="graph-param-grid">
                <label>
                  <span>{t("graph.partials")}</span>
                  <NumberField
                    value={Math.round(paramOf(node, "partials", 10))}
                    min={1}
                    max={16}
                    step={1}
                    precision={0}
                    disabled={busy}
                    onchange={(value) =>
                      onParamChange(owner.id, node.id, "partials", value)}
                  />
                </label>
                <label>
                  <span>{t("graph.decay")}</span>
                  <NumberField
                    value={paramOf(node, "decay", 1.2)}
                    min={0.1}
                    max={8}
                    step={0.1}
                    precision={1}
                    suffix="s"
                    disabled={busy}
                    onchange={(value) =>
                      onParamChange(owner.id, node.id, "decay", value)}
                  />
                </label>
                <label>
                  <span>{t("graph.brightness")}</span>
                  <NumberField
                    value={Math.round(paramOf(node, "brightness", 0.5) * 100)}
                    min={0}
                    max={100}
                    step={1}
                    precision={0}
                    suffix="%"
                    disabled={busy}
                    onchange={(value) =>
                      onParamChange(owner.id, node.id, "brightness", value / 100)}
                  />
                </label>
                <label>
                  <span>{t("graph.hammer")}</span>
                  <NumberField
                    value={Math.round(paramOf(node, "hammer", 0.2) * 100)}
                    min={0}
                    max={100}
                    step={1}
                    precision={0}
                    suffix="%"
                    disabled={busy}
                    onchange={(value) =>
                      onParamChange(owner.id, node.id, "hammer", value / 100)}
                  />
                </label>
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
            {:else if node.kind === "plugin"}
              <div class="graph-param-row">
                <span>{t("graph.plugin")}</span>
                <strong>{node.plugin?.id ?? "未配置"}</strong>
              </div>
            {:else}
              <div class="graph-param-row">
                <span>{t("graph.terminal")}</span>
              </div>
            {/if}
          </div>
        {/each}
      </div>

      {#if nodeMenu}
        <div
          class="graph-context-menu"
          style:left={`${nodeMenu.x}px`}
          style:top={`${nodeMenu.y}px`}
          role="menu"
          tabindex="-1"
          onmousedown={(event) => event.stopPropagation()}
        >
          <button role="menuitem" disabled={busy} onclick={duplicateFromMenu}>
            {t("graph.duplicate")}
          </button>
          <button
            role="menuitem"
            class="danger"
            disabled={busy}
            onclick={deleteFromMenu}
          >
            {t("graph.delete")}
          </button>
        </div>
      {/if}

      {#if addMenu}
        <div
          class="graph-add-menu"
          style:left={`${addMenu.x}px`}
          style:top={`${addMenu.y}px`}
          role="menu"
          tabindex="-1"
          onmousedown={(event) => event.stopPropagation()}
        >
          <button
            role="menuitem"
            disabled={busy || hasAdsr}
            onclick={() => addNode("amp.adsr")}
          >
            {t("graph.addAdsr")}
          </button>
          <button
            role="menuitem"
            disabled={busy}
            onclick={() => addNode("filter.lowpass")}
          >
            {t("graph.addFilter")}
          </button>
          <button
            role="menuitem"
            disabled={busy}
            onclick={() => addNode("mix")}
          >
            {t("graph.addMix")}
          </button>
          <button
            role="menuitem"
            disabled={busy}
            onclick={() => addNode("gain")}
          >
            {t("graph.addGain")}
          </button>
        </div>
      {/if}
    </div>

    <div class="section-foot">
      <span>{t("graph.foot")}</span>
      <span>{t("graph.direction")}</span>
    </div>
  {:else}
    <p class="empty">{t("graph.noTrack")}</p>
  {/if}
</section>
