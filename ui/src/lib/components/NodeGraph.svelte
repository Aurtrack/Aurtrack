<script lang="ts">
  import { tick } from "svelte";
  import type {
    GraphConnection,
    GraphNode,
    NodeGroup,
    NodeTypeInfo,
    Track,
    VoiceFrame,
  } from "../api";
  import { t } from "../i18n.svelte";
  import { session } from "../session.svelte";
  import NodeAddMenu from "./NodeAddMenu.svelte";
  import NumberField from "./NumberField.svelte";
  import VoiceGraphEditor from "./VoiceGraphEditor.svelte";
  import {
    nodeTypeLabel,
    paramLabel,
    socketLabel,
  } from "../nodeLabels";
  import {
    NODE_BODY_TOP,
    NODE_HEADER,
    NODE_ROW,
    NODE_WIDTH,
    collapsedNodeHeight,
    collapsedPortY,
    expandedNodeHeight,
    expandedPortY,
  } from "./nodeLayout";

  interface Props {
    track: Track | null;
    busy: boolean;
    onAddNode: (
      trackId: string,
      kind: string,
      x: number,
      y: number,
      group?: string,
    ) => void | Promise<string | undefined>;
    onConnect: (
      trackId: string,
      fromNodeId: string,
      fromPort: string,
      toNodeId: string,
      toPort: string,
    ) => void | Promise<unknown>;
    onDisconnect: (
      trackId: string,
      fromNodeId: string,
      fromPort: string,
      toNodeId: string,
      toPort: string,
    ) => void | Promise<unknown>;
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
    onFlags?: (
      trackId: string,
      nodeId: string,
      patch: { muted?: boolean; collapsed?: boolean },
    ) => void | Promise<unknown>;
    onFrames?: (
      trackId: string,
      frames: Record<string, VoiceFrame>,
      assignments: Record<string, string | null>,
    ) => void | Promise<unknown>;
    onOpenAudioGroup?: (node: GraphNode) => void;
    onMakeAudioGroup?: (
      trackId: string,
      nodeIds: string[],
    ) => void | Promise<unknown>;
    onUngroupAudioGroup?: (
      trackId: string,
      nodeId: string,
    ) => void | Promise<unknown>;
    onExitGroup?: () => void;
    canExitGroup?: boolean;
    compact?: boolean;
    inactive?: boolean;
    autoFrame?: boolean;
    tidyOnOpen?: boolean;
    isNodeLocked?: (node: GraphNode) => boolean;
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
    onFlags,
    onFrames,
    onOpenAudioGroup,
    onMakeAudioGroup,
    onUngroupAudioGroup,
    onExitGroup,
    canExitGroup = false,
    compact = false,
    inactive = false,
    autoFrame = false,
    tidyOnOpen = false,
    isNodeLocked,
  }: Props = $props();

  let audioTypeById = $derived(
    new Map(session.audioNodeTypes.map((nodeType) => [nodeType.id, nodeType])),
  );
  let addNodeTypes = $derived(
    session.audioNodeTypes.filter(
      (nodeType) =>
        nodeType.category !== "Instrument" &&
        nodeType.category !== "Output" &&
        nodeType.category !== "Group" &&
        nodeType.category !== "Interface" &&
        nodeType.id !== "audio.group" &&
        nodeType.id !== "audio.group.instance" &&
        !(nodeType.id === "audio.adsr" && hasAdsr),
    ),
  );

  function nodeTypeId(node: GraphNode): string {
    if (node.type_id) return node.type_id;
    if (node.kind === "amp.adsr") return "audio.adsr";
    if (node.kind === "filter.lowpass") return "audio.filter.lowpass";
    if (node.kind === "mix") return "audio.mix";
    if (node.kind === "gain") return "audio.gain";
    if (node.kind === "plugin") return "audio.plugin";
    if (node.kind === "voice.group") return "audio.group";
    if (node.kind === "instrument.sine") return "audio.instrument.sine";
    return "audio.output";
  }

  function nodeType(node: GraphNode): NodeTypeInfo | undefined {
    return audioTypeById.get(nodeTypeId(node));
  }

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
    moved: boolean;
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

  const CARD_WIDTH = NODE_WIDTH;
  const COLUMN_GAP = 110;
  const MIN_SCALE = 0.25;
  const MAX_SCALE = 2.5;

  let board = $state<HTMLDivElement>();
  let view = $state({ x: 20, y: 30, scale: 0.8 });
  let selected = $state<Set<string>>(new Set());
  let draft = $state<Record<string, Point>>({});
  let nodeDrag = $state<NodeDrag | null>(null);
  let panDrag = $state<PanDrag | null>(null);
  let marquee = $state<Marquee | null>(null);
  let lasso = $state<{ points: Point[]; subtract: boolean } | null>(null);
  let wireDrag = $state<WireDrag | null>(null);
  let pendingWire = $state<WireDrag | null>(null);
  let insertTarget = $state<GraphConnection | null>(null);
  let clipboard = $state<string[]>([]);
  let cutLine = $state<Point[] | null>(null);
  let circleMode = $state(false);
  let circleRadius = $state(48);
  let circlePos = $state<Point | null>(null);
  let circleDragging = false;
  let frameDraft = $state<Record<string, VoiceFrame> | null>(null);
  let frameDrag = $state<{
    id: string;
    startX: number;
    startY: number;
    origin: [number, number];
    children: Record<string, [number, number]>;
  } | null>(null);
  let frameResize = $state<{
    id: string;
    startX: number;
    startY: number;
    size: [number, number];
  } | null>(null);
  let wireError = $state("");
  let wireHover = $state<{ node: string; port: string } | null>(null);
  let active = $state("");
  let sidebarOpen = $state(false);
  let addMenu = $state<AddMenu | null>(null);
  let nodeMenu = $state<NodeMenu | null>(null);
  let canvasMenu = $state<Point | null>(null);
  type VoicePathEntry = { id: string; name: string };
  let voicePath = $state<VoicePathEntry[]>([]);
  let spaceHeld = $state(false);
  let lastPointer = $state<Point>({ x: 0, y: 0 });

  let graph = $derived(track?.graph ?? null);
  let nodes = $derived(graph ? Object.values(graph.nodes) : ([] as GraphNode[]));
  let editingGroup = $derived.by(() => {
    const current = voicePath.at(-1);
    return current ? session.nodeGroups[current.id] : undefined;
  });
  let connections = $derived(graph?.connections ?? ([] as GraphConnection[]));
  let frames = $derived(
    Object.values(frameDraft ?? graph?.frames ?? {}),
  );
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
    frameDraft = null;
    addMenu = null;
    nodeMenu = null;
    view = { x: 20, y: 30, scale: 0.8 };
  });

  let autoFramedFor = "";

  $effect(() => {
    const id = track?.id ?? "";
    if (!autoFrame || !id || id === autoFramedFor || nodes.length === 0) return;
    autoFramedFor = id;
    void tick().then(() => {
      requestAnimationFrame(() => frameAll());
    });
  });

  let tidiedFor = "";

  $effect(() => {
    const owner = track;
    if (!tidyOnOpen || !owner || owner.id === tidiedFor || nodes.length < 2) {
      return;
    }
    tidiedFor = owner.id;
    const positions = Object.values(nodePositions);
    const overlapping = positions.some((left, index) =>
      positions
        .slice(index + 1)
        .some(
          (right) =>
            left.x < right.x + right.w - 2 &&
            left.x + left.w - 2 > right.x &&
            left.y < right.y + right.h - 2 &&
            left.y + left.h - 2 > right.y,
        ),
    );
    if (!overlapping) return;
    const layout = buildLayout(nodes, connections, true);
    const placed = nodes.map((node) => {
      const position = layout.positions[node.id];
      return {
        id: node.id,
        x: position?.x ?? 30,
        y: position?.y ?? 30,
      };
    });
    void Promise.resolve(onPlace(owner.id, placed)).then(() =>
      tick().then(() => {
        requestAnimationFrame(() => frameAll());
      }),
    );
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

  function groupHeaderHeight(node: GraphNode): number {
    if (
      nodeTypeId(node) === "audio.group.instance" ||
      nodeTypeId(node) === "audio.group"
    ) {
      return 18;
    }
    return 0;
  }

  function groupPorts(node: GraphNode, side: "inputs" | "outputs") {
    const type = nodeTypeId(node);
    if (type === "audio.group") {
      if (side === "outputs") return undefined;
      const group = node.group ? session.nodeGroups[node.group] : undefined;
      if (!group) return [];
      return group.interface.inputs.map((socket) => ({
        id: socket.id,
        name: socket.name,
        type: socket.type,
        default: socket.default,
        panel: socket.panel,
      }));
    }
    if (type === "audio.group.input") {
      const group = track ? session.audioGroups[track.id] : undefined;
      if (side !== "outputs") return [];
      return (group?.interface.inputs ?? []).map((socket) => ({
        id: socket.id,
        name: socket.name,
        type: socket.type,
        default: socket.default,
        panel: socket.panel,
      }));
    }
    if (type === "audio.group.output") {
      const group = track ? session.audioGroups[track.id] : undefined;
      if (side !== "inputs") return [];
      return (group?.interface.outputs ?? []).map((socket) => ({
        id: socket.id,
        name: socket.name,
        type: socket.type,
        default: socket.default,
        panel: socket.panel,
      }));
    }
    if (type !== "audio.group.instance") return undefined;
    const group = node.group ? session.audioGroups[node.group] : undefined;
    if (!group) return [];
    return (side === "inputs" ? group.interface.inputs : group.interface.outputs).map(
      (socket) => ({
        id: socket.id,
        name: socket.name,
        type: socket.type,
        default: socket.default,
        panel: socket.panel,
      }),
    );
  }

  function nodeInputs(node: GraphNode) {
    return groupPorts(node, "inputs") ?? nodeType(node)?.inputs ?? [];
  }

  function nodeOutputs(node: GraphNode) {
    return groupPorts(node, "outputs") ?? nodeType(node)?.outputs ?? [];
  }

  function matchingParam(node: GraphNode, portId: string) {
    const declared = nodeType(node)?.params.find(
      (param) => param.name === portId,
    );
    if (declared) return declared;
    const type = nodeTypeId(node);
    const group =
      type === "audio.group.instance"
        ? node.group
          ? session.audioGroups[node.group]
          : undefined
        : type === "audio.group" && node.group
          ? session.nodeGroups[node.group]
          : undefined;
    if (!group) return undefined;
    const socket = group.interface.inputs.find(
      (candidate) => candidate.id === portId,
    );
    if (!socket || socket.type === "audio" || socket.type === "note")
      return undefined;
    return {
      name: socket.id,
      default: socket.default ?? 0,
      minimum: socket.minimum ?? -Infinity,
      maximum: socket.maximum ?? Infinity,
      step: socket.step ?? 0.01,
      panel: socket.panel,
    };
  }

  function isLinked(node: GraphNode, portId: string): boolean {
    return connections.some(
      (connection) =>
        connection.to.node === node.id && connection.to.port === portId,
    );
  }

  function extraParams(node: GraphNode) {
    const ports = new Set(nodeInputs(node).map((port) => port.id));
    return (nodeType(node)?.params ?? []).filter(
      (param) => !ports.has(param.name),
    );
  }

  function infoRows(node: GraphNode): number {
    return node.plugin ? 1 : 0;
  }

  function rowCount(node: GraphNode): number {
    return (
      Math.max(nodeInputs(node).length, nodeOutputs(node).length) +
      extraParams(node).length +
      infoRows(node)
    );
  }

  function collapsedRows(node: GraphNode): number {
    return Math.max(1, Math.max(nodeInputs(node).length, nodeOutputs(node).length));
  }

  function portY(node: GraphNode, index: number): number {
    if (node.collapsed) return collapsedPortY(index);
    return expandedPortY(index, groupHeaderHeight(node));
  }

  function portTop(node: GraphNode, index: number): number {
    return node.collapsed
      ? portY(node, index)
      : portY(node, index) - NODE_ROW / 2;
  }

  function nodeHeight(node: GraphNode): number {
    if (node.collapsed) return collapsedNodeHeight(collapsedRows(node));
    return expandedNodeHeight(rowCount(node), groupHeaderHeight(node));
  }

  function nodeSize(node: GraphNode) {
    return { w: CARD_WIDTH, h: nodeHeight(node) };
  }

  function buildLayout(
    nodeList: GraphNode[],
    edges: GraphConnection[],
    force = false,
  ): Layout {
    const layoutNodes = force
      ? nodeList
      : nodeList.filter((node) => !node.position);
    if (layoutNodes.length === 0) {
      return { positions: {}, width: 0, height: 0 };
    }
    const byId = new Map(layoutNodes.map((node) => [node.id, node]));
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
    for (const node of layoutNodes) {
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
        const h = nodeHeight(node);
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
    if (nodeTypeId(node) === "audio.group") {
      return groupFor(node)?.name ?? node.group ?? t("graph.group");
    }
    if (nodeTypeId(node) === "audio.group.instance") {
      return (
        (node.group ? session.audioGroups[node.group]?.name : undefined) ??
        node.group ??
        t("graph.group")
      );
    }
    const info = nodeType(node);
    if (info) return nodeTypeLabel(info).toUpperCase();
    if (node.kind === "instrument.silence") return t("graph.silence");
    if (node.kind === "voice.group") return t("graph.group");
    if (node.kind === "output") return t("graph.output");
    return node.kind.toUpperCase();
  }

  function groupFor(node: GraphNode): NodeGroup | undefined {
    if (!node.group) return undefined;
    return session.nodeGroups[node.group];
  }

  function openVoiceGroup(node: GraphNode) {
    const group = groupFor(node);
    if (!group) return;
    voicePath = [{ id: group.id, name: group.name }];
  }

  function enterVoiceGroup(groupId: string) {
    const group = session.nodeGroups[groupId];
    if (!group) return;
    voicePath = [...voicePath, { id: group.id, name: group.name }];
  }

  function navigateVoice(index: number) {
    if (index < 0) {
      voicePath = [];
      return;
    }
    voicePath = voicePath.slice(0, index + 1);
  }

  function isDevice(node: GraphNode): boolean {
    const category = nodeType(node)?.category;
    return (
      category !== undefined &&
      category !== "Instrument" &&
      category !== "Output" &&
      category !== "Input"
    );
  }

  function paramOf(node: GraphNode, name: string, fallback: number): number {
    return node.params[name] ?? fallback;
  }

  type InputPort = { port: string; y: number; ty: string };

  const socketColors: Record<string, string> = {
    float: "#a1a1a1",
    gate: "#cca6d6",
    audio: "#63c763",
    note: "#d99a4e",
  };

  function wireDragColor(): string {
    if (!wireDrag) return socketColors.audio ?? "#63c763";
    if (wireDrag.side === "out") {
      return (
        socketColors[socketType(wireDrag.node, "out", wireDrag.port) ?? "audio"] ??
        "#63c763"
      );
    }
    return (
      socketColors[socketType(wireDrag.node, "in", wireDrag.port) ?? "audio"] ??
      "#63c763"
    );
  }

  function wireColor(connection: GraphConnection): string {
    const type = socketType(
      connection.from.node,
      "out",
      connection.from.port,
    );
    return socketColors[type ?? "audio"] ?? "#63c763";
  }

  function socketType(
    nodeId: string,
    side: "in" | "out",
    port: string,
  ): string | undefined {
    const node = graph?.nodes[nodeId];
    if (!node) return undefined;
    const info = nodeType(node);
    const sockets = side === "in" ? info?.inputs : info?.outputs;
    return sockets?.find((socket) => socket.id === port)?.type;
  }

  function sidebarParams(node: GraphNode) {
    const params = [...(nodeType(node)?.params ?? [])];
    const type = nodeTypeId(node);
    if (type === "audio.group.instance" || type === "audio.group") {
      const group =
        type === "audio.group"
          ? node.group
            ? session.nodeGroups[node.group]
            : undefined
          : node.group
            ? session.audioGroups[node.group]
            : undefined;
      for (const socket of group?.interface.inputs ?? []) {
        if (socket.type === "audio" || socket.type === "note") continue;
        const param = matchingParam(node, socket.id);
        if (param) params.push(param);
      }
    }
    return params.filter((param) => !isLinked(node, param.name));
  }

  function sidebarSections(node: GraphNode) {
    const sections = new Map<string, ReturnType<typeof sidebarParams>>();
    for (const param of sidebarParams(node)) {
      const panel = param.panel?.trim() ?? "";
      const list = sections.get(panel) ?? [];
      list.push(param);
      sections.set(panel, list);
    }
    return [...sections.entries()];
  }

  function inputPorts(node: GraphNode): InputPort[] {
    return nodeInputs(node).map((input, index) => ({
      port: input.id,
      y: portY(node, index),
      ty: input.type,
    }));
  }

  function outputPoint(nodeId: string): Point | null {
    const position = nodePositions[nodeId];
    const node = graph?.nodes[nodeId];
    if (!position || !node) return null;
    return {
      x: position.x + position.w,
      y: position.y + portY(node, 0),
    };
  }

  function inputPoint(nodeId: string, portName: string): Point | null {
    const position = nodePositions[nodeId];
    const node = graph?.nodes[nodeId];
    if (!position || !node) return null;
    const port = inputPorts(node).find(
      (candidate) => candidate.port === portName,
    );
    return {
      x: position.x,
      y: position.y + (port?.y ?? portY(node, 0)),
    };
  }

  function pointSegmentDistance(
    point: Point,
    from: Point,
    to: Point,
  ): number {
    const dx = to.x - from.x;
    const dy = to.y - from.y;
    const length = dx * dx + dy * dy;
    const t =
      length === 0
        ? 0
        : Math.max(
            0,
            Math.min(
              1,
              ((point.x - from.x) * dx + (point.y - from.y) * dy) / length,
            ),
          );
    return Math.hypot(
      point.x - (from.x + dx * t),
      point.y - (from.y + dy * t),
    );
  }

  function wireSamples(
    connection: GraphConnection,
    steps = 24,
  ): Point[] {
    const from = outputPoint(connection.from.node);
    const to = inputPoint(connection.to.node, connection.to.port);
    if (!from || !to) return [];
    const bend = Math.max(48, Math.abs(to.x - from.x) * 0.4);
    const points: Point[] = [];
    for (let step = 0; step <= steps; step += 1) {
      const t = step / steps;
      const inverse = 1 - t;
      points.push({
        x:
          inverse ** 3 * from.x +
          3 * inverse ** 2 * t * (from.x + bend) +
          3 * inverse * t ** 2 * (to.x - bend) +
          t ** 3 * to.x,
        y:
          inverse ** 3 * from.y +
          3 * inverse ** 2 * t * from.y +
          3 * inverse * t ** 2 * to.y +
          t ** 3 * to.y,
      });
    }
    return points;
  }

  function segmentsIntersect(
    a: Point,
    b: Point,
    c: Point,
    d: Point,
  ): boolean {
    const cross = (p: Point, q: Point, r: Point) =>
      (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x);
    const d1 = cross(a, b, c);
    const d2 = cross(a, b, d);
    const d3 = cross(c, d, a);
    const d4 = cross(c, d, b);
    return d1 * d2 <= 0 && d3 * d4 <= 0;
  }

  function cutWire(connection: GraphConnection, line: Point[]): boolean {
    const samples = wireSamples(connection);
    for (let index = 1; index < line.length; index += 1) {
      const a = line[index - 1];
      const b = line[index];
      if (!a || !b) continue;
      for (let step = 1; step < samples.length; step += 1) {
        const c = samples[step - 1];
        const d = samples[step];
        if (c && d && segmentsIntersect(a, b, c, d)) return true;
      }
    }
    return false;
  }

  function selectInCircle(center: Point, subtract: boolean) {
    const next = new Set(selected);
    for (const node of nodes) {
      const position = nodePositions[node.id];
      if (!position) continue;
      const point = {
        x: view.x + (position.x + position.w / 2) * view.scale,
        y: view.y + (position.y + position.h / 2) * view.scale,
      };
      if (Math.hypot(point.x - center.x, point.y - center.y) <= circleRadius) {
        if (subtract) next.delete(node.id);
        else next.add(node.id);
      }
    }
    selected = next;
  }

  function findInsertTarget(nodeId: string): GraphConnection | null {
    const node = graph?.nodes[nodeId];
    const position = nodePositions[nodeId];
    if (!node || !position) return null;
    const info = nodeType(node);
    if (!info || info.inputs.length === 0 || info.outputs.length === 0) {
      return null;
    }
    const center = {
      x: position.x + position.w / 2,
      y: position.y + position.h / 2,
    };
    for (const connection of connections) {
      if (
        connection.from.node === nodeId ||
        connection.to.node === nodeId
      ) {
        continue;
      }
      if (connection.from.port !== "out") continue;
      const from = outputPoint(connection.from.node);
      const to = inputPoint(connection.to.node, connection.to.port);
      if (!from || !to) continue;
      const bend = Math.max(48, Math.abs(to.x - from.x) * 0.4);
      let closest = Infinity;
      let previous = from;
      for (let step = 1; step <= 18; step += 1) {
        const t = step / 18;
        const inverse = 1 - t;
        const point = {
          x:
            inverse ** 3 * from.x +
            3 * inverse ** 2 * t * (from.x + bend) +
            3 * inverse * t ** 2 * (to.x - bend) +
            t ** 3 * to.x,
          y:
            inverse ** 3 * from.y +
            3 * inverse ** 2 * t * from.y +
            3 * inverse * t ** 2 * to.y +
            t ** 3 * to.y,
        };
        closest = Math.min(closest, pointSegmentDistance(center, previous, point));
        previous = point;
      }
      const type = socketType(
        connection.from.node,
        "out",
        connection.from.port,
      );
      if (
        closest <= 26 &&
        type &&
        info.inputs.some((port) => port.type === type) &&
        info.outputs.some((port) => port.type === type)
      ) {
        return connection;
      }
    }
    return null;
  }

  async function insertNodeOnConnection(
    nodeId: string,
    connection: GraphConnection,
  ) {
    if (!track) return;
    const node = graph?.nodes[nodeId];
    if (!node) return;
    const info = nodeType(node);
    if (!info) return;
    const type = socketType(
      connection.from.node,
      "out",
      connection.from.port,
    );
    const input = info.inputs.find((port) => port.type === type);
    const output = info.outputs.find((port) => port.type === type);
    if (!input || !output) return;
    await onDisconnect(
      track.id,
      connection.from.node,
      connection.from.port,
      connection.to.node,
      connection.to.port,
    );
    await onConnect(
      track.id,
      connection.from.node,
      connection.from.port,
      nodeId,
      input.id,
    );
    await onConnect(
      track.id,
      nodeId,
      output.id,
      connection.to.node,
      connection.to.port,
    );
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
    if (circleMode) {
      circleRadius = Math.max(
        12,
        Math.min(240, circleRadius - event.deltaY * 0.08),
      );
      return;
    }
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

  function openCanvasMenu(event: MouseEvent) {
    if (event.button !== 2 || event.ctrlKey) return;
    const target = event.target as Element | null;
    if (target?.closest(".graph-node, .graph-wire-hit")) return;
    event.preventDefault();
    canvasMenu = { x: event.clientX, y: event.clientY };
    addMenu = null;
    nodeMenu = null;
  }

  function boardMouseDown(event: MouseEvent) {
    if (circleMode) {
      event.preventDefault();
      if (event.button === 2) {
        circleMode = false;
        circleDragging = false;
        circlePos = null;
        return;
      }
      if (event.button !== 0) return;
      const point = pointer(event);
      if (!point) return;
      circlePos = point;
      circleDragging = true;
      selectInCircle(point, event.shiftKey);
      return;
    }
    if (event.button === 2 && event.ctrlKey) {
      event.preventDefault();
      const point = pointer(event);
      if (point) {
        cutLine = [point];
        canvasMenu = null;
        addMenu = null;
        nodeMenu = null;
      }
      return;
    }
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
    if (
      target?.closest(
        ".graph-node, .graph-wire-hit, .graph-add-menu, .graph-context-menu",
      )
    ) {
      return;
    }
    if (event.ctrlKey && event.altKey) {
      const screen = pointer(event);
      if (screen) {
        lasso = { points: [screen], subtract: event.shiftKey };
      }
      event.preventDefault();
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
    canvasMenu = null;
    event.preventDefault();
  }

  function selectNode(node: GraphNode, event: MouseEvent) {
    addMenu = null;
    if (event.button !== 0) return;
    active = node.id;
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
    if (circleMode) return;
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

  function toggleNodeFlag(flag: "muted" | "collapsed") {
    if (!track) return;
    for (const node of nodes) {
      if (!selected.has(node.id) || isNodeLocked?.(node)) continue;
      const current = Boolean(node[flag]);
      const patch = { [flag]: !current };
      if (onFlags) void onFlags(track.id, node.id, patch);
      else void session.setNodeFlags(track.id, node.id, patch);
    }
  }

  function saveFrames(
    frames: Record<string, VoiceFrame>,
    assignments: Record<string, string | null> = {},
  ) {
    if (!track) return;
    if (onFrames) void onFrames(track.id, frames, assignments);
    else void session.setNodeFrames(track.id, frames, assignments);
  }

  function createFrame() {
    if (!track) return;
    const members = nodes.filter((node) => selected.has(node.id));
    let left = Infinity;
    let top = Infinity;
    let right = -Infinity;
    let bottom = -Infinity;
    for (const node of members) {
      const position = nodePositions[node.id];
      if (!position) continue;
      left = Math.min(left, position.x);
      top = Math.min(top, position.y);
      right = Math.max(right, position.x + position.w);
      bottom = Math.max(bottom, position.y + position.h);
    }
    if (!Number.isFinite(left)) {
      const rect = board?.getBoundingClientRect();
      const center = rect
        ? worldPoint({
            clientX: rect.left + rect.width / 2,
            clientY: rect.top + rect.height / 2,
          })
        : null;
      left = (center?.x ?? 200) - 160;
      top = (center?.y ?? 200) - 100;
      right = left + 320;
      bottom = top + 200;
    }
    const frame: VoiceFrame = {
      id: `frame-${Date.now().toString(36)}`,
      name: t("voice.frame"),
      position: [left - 24, top - 42],
      size: [right - left + 48, bottom - top + 66],
    };
    const assignments: Record<string, string | null> = {};
    for (const node of members) assignments[node.id] = frame.id;
    saveFrames(
      { ...(graph?.frames ?? {}), [frame.id]: frame },
      assignments,
    );
  }

  function deleteFrame(frame: VoiceFrame) {
    if (!track || !window.confirm(t("voice.confirmDeleteFrame"))) return;
    const next = { ...(graph?.frames ?? {}) };
    delete next[frame.id];
    const assignments: Record<string, string | null> = {};
    for (const node of nodes) {
      if (node.frame === frame.id) assignments[node.id] = null;
    }
    saveFrames(next, assignments);
  }

  function frameMouseDown(event: MouseEvent, frame: VoiceFrame) {
    if (circleMode) return;
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    const children: Record<string, [number, number]> = {};
    for (const node of nodes) {
      if (node.frame !== frame.id) continue;
      const position = nodePositions[node.id];
      if (position) children[node.id] = [position.x, position.y];
    }
    frameDraft = { ...(graph?.frames ?? {}) };
    frameDrag = {
      id: frame.id,
      startX: event.clientX,
      startY: event.clientY,
      origin: [...frame.position],
      children,
    };
  }

  function frameResizeDown(event: MouseEvent, frame: VoiceFrame) {
    if (circleMode) return;
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    frameDraft = { ...(graph?.frames ?? {}) };
    frameResize = {
      id: frame.id,
      startX: event.clientX,
      startY: event.clientY,
      size: [...frame.size],
    };
  }

  function openNodeMenu(event: MouseEvent, node: GraphNode) {
    if (!isDevice(node)) return;
    event.preventDefault();
    event.stopPropagation();
    if (!selected.has(node.id)) selected = new Set([node.id]);
    active = node.id;
    addMenu = null;
    canvasMenu = null;
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
    if (circleMode) return;
    if (event.button !== 0 || busy || spaceHeld) return;
    event.preventDefault();
    event.stopPropagation();
    const point = worldPoint(event);
    if (!point) return;
    if (event.ctrlKey && side === "in" && !wireDrag) {
      const existing = connections.find(
        (connection) =>
          connection.to.node === node.id && connection.to.port === port,
      );
      if (existing && track) {
        void onDisconnect(
          track.id,
          existing.from.node,
          existing.from.port,
          existing.to.node,
          existing.to.port,
        );
        wireDrag = {
          side: "out",
          node: existing.from.node,
          port: existing.from.port,
          x: point.x,
          y: point.y,
          moved: false,
        };
        wireHover = null;
        return;
      }
    }
    wireDrag = {
      side,
      node: node.id,
      port,
      x: point.x,
      y: point.y,
      moved: false,
    };
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
    if (circleMode) {
      const point = pointer(event);
      if (point) {
        circlePos = point;
        if (circleDragging) selectInCircle(point, event.shiftKey);
      }
      return;
    }
    if (cutLine) {
      const point = pointer(event);
      if (point) {
        const previous = cutLine.at(-1);
        if (
          !previous ||
          Math.hypot(point.x - previous.x, point.y - previous.y) > 2
        ) {
          cutLine = [...cutLine, point];
        }
      }
      return;
    }
    if (lasso) {
      const point = pointer(event);
      if (point) {
        const previous = lasso.points.at(-1);
        if (
          !previous ||
          Math.hypot(point.x - previous.x, point.y - previous.y) > 2
        ) {
          lasso = { ...lasso, points: [...lasso.points, point] };
        }
      }
      return;
    }
    if (panDrag) {
      view = {
        ...view,
        x: panDrag.viewX + event.clientX - panDrag.startX,
        y: panDrag.viewY + event.clientY - panDrag.startY,
      };
      return;
    }
    if (frameDrag) {
      const dx = (event.clientX - frameDrag.startX) / view.scale;
      const dy = (event.clientY - frameDrag.startY) / view.scale;
      if (frameDraft?.[frameDrag.id]) {
        const frame = frameDraft[frameDrag.id];
        if (frame) {
          frameDraft = {
            ...frameDraft,
            [frameDrag.id]: {
              ...frame,
              position: [
                frameDrag.origin[0] + dx,
                frameDrag.origin[1] + dy,
              ],
            },
          };
        }
      }
      const next = { ...draft };
      for (const [id, origin] of Object.entries(frameDrag.children)) {
        next[id] = { x: origin[0] + dx, y: origin[1] + dy };
      }
      draft = next;
      return;
    }
    if (frameResize) {
      if (frameDraft?.[frameResize.id]) {
        const frame = frameDraft[frameResize.id];
        if (frame) {
          frameDraft = {
            ...frameDraft,
            [frameResize.id]: {
              ...frame,
              size: [
                Math.max(
                  160,
                  frameResize.size[0] +
                    (event.clientX - frameResize.startX) / view.scale,
                ),
                Math.max(
                  100,
                  frameResize.size[1] +
                    (event.clientY - frameResize.startY) / view.scale,
                ),
              ],
            },
          };
        }
      }
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
      insertTarget =
        nodeDrag.ids.length === 1
          ? findInsertTarget(nodeDrag.ids[0] ?? "")
          : null;
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
      if (point) wireDrag = { ...wireDrag, x: point.x, y: point.y, moved: true };
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

  function pointInPolygon(point: Point, polygon: Point[]): boolean {
    let inside = false;
    for (let index = 0, previous = polygon.length - 1; index < polygon.length; previous = index++) {
      const current = polygon[index];
      const before = polygon[previous];
      if (!current || !before) continue;
      const intersects =
        current.y > point.y !== before.y > point.y &&
        point.x <
          ((before.x - current.x) * (point.y - current.y)) /
            (before.y - current.y) +
            current.x;
      if (intersects) inside = !inside;
    }
    return inside;
  }

  async function handleMouseUp() {
    if (circleMode) {
      circleDragging = false;
      return;
    }
    if (cutLine) {
      const line = cutLine;
      cutLine = null;
      if (!track || line.length < 2) return;
      const doomed = connections.filter((connection) =>
        cutWire(connection, line),
      );
      for (const connection of doomed) {
        await onDisconnect(
          track.id,
          connection.from.node,
          connection.from.port,
          connection.to.node,
          connection.to.port,
        );
      }
      return;
    }
    if (frameDrag || frameResize) {
      const frames = frameDraft;
      const children = frameDrag?.children ?? {};
      frameDrag = null;
      frameResize = null;
      frameDraft = null;
      if (track && frames) {
        saveFrames(frames);
        const placed = Object.keys(children)
          .map((id) => {
            const point = draft[id];
            return point ? { id, x: point.x, y: point.y } : null;
          })
          .filter((item): item is { id: string; x: number; y: number } =>
            Boolean(item),
          );
        if (placed.length > 0) {
          void onPlace(track.id, placed);
        }
      }
      draft = {};
      return;
    }
    if (lasso) {
      const area = lasso;
      lasso = null;
      if (area.points.length > 2) {
        const next = new Set(selected);
        for (const node of nodes) {
          const position = nodePositions[node.id];
          if (!position) continue;
          const center = {
            x: view.x + (position.x + position.w / 2) * view.scale,
            y: view.y + (position.y + position.h / 2) * view.scale,
          };
          if (pointInPolygon(center, area.points)) {
            if (area.subtract) next.delete(node.id);
            else next.add(node.id);
          }
        }
        selected = next;
      }
      return;
    }
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
      if (
        drag.moved &&
        drag.ids.length === 1 &&
        insertTarget
      ) {
        void insertNodeOnConnection(drag.ids[0] ?? "", insertTarget);
      }
      insertTarget = null;
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
      if (!track) return;
      if (!target) {
        if (drag.moved) {
          pendingWire = drag;
          addMenu = {
            x: Math.max(10, lastPointer.x),
            y: Math.max(10, lastPointer.y),
          };
        }
        return;
      }
      const fromNode = drag.side === "out" ? drag.node : target.node;
      const fromPort = drag.side === "out" ? drag.port : "out";
      const toNode = drag.side === "out" ? target.node : drag.node;
      const toPort = drag.side === "out" ? target.port : drag.port;
      const sourceType = socketType(fromNode, "out", fromPort);
      const targetType = socketType(toNode, "in", toPort);
      if (sourceType && targetType && sourceType !== targetType) {
        wireError = `端口类型不匹配：${sourceType} → ${targetType}`;
        return;
      }
      wireError = "";
      onConnect(track.id, fromNode, fromPort, toNode, toPort);
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

  function copySelected() {
    clipboard = selectedPaths();
  }

  async function pasteClipboard() {
    if (!track || clipboard.length === 0) return;
    const copied = await onDuplicate(clipboard);
    if (copied && copied.length > 0) {
      selected = new Set(copied.map((item) => item.node_id));
      active = copied.at(-1)?.node_id ?? "";
    }
  }

  async function makeAudioGroup() {
    if (!track || !onMakeAudioGroup) return;
    const nodeIds = nodes
      .filter((node) => selected.has(node.id) && !isNodeLocked?.(node))
      .map((node) => node.id);
    if (nodeIds.length === 0) return;
    await onMakeAudioGroup(track.id, nodeIds);
    selected = new Set();
  }

  async function ungroupAudioGroup() {
    if (!track || !onUngroupAudioGroup) return;
    const node = nodes.find(
      (candidate) =>
        selected.has(candidate.id) &&
        nodeTypeId(candidate) === "audio.group.instance",
    );
    if (!node) return;
    await onUngroupAudioGroup(track.id, node.id);
    selected = new Set();
  }

  function deleteSelected() {
    const paths = selectedPaths().filter((path) => {
      const id = path.split("/").at(-1) ?? "";
      const node = graph?.nodes[id];
      return node && !isNodeLocked?.(node);
    });
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
          !isNodeLocked?.(node) &&
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
    if (!(element instanceof HTMLElement)) return false;
    const editing =
      element instanceof HTMLInputElement ||
      element instanceof HTMLTextAreaElement ||
      element instanceof HTMLSelectElement ||
      element.isContentEditable;
    if (!editing) return false;
    return Boolean(
      element.closest(".graph-section, .node-add-menu, .node-sidebar"),
    );
  }

  function handleKeyDown(event: KeyboardEvent) {
    if (inactive || editingGroup) return;
    if ((event.code === "Space" || event.key === " ") && !isEditing()) {
      spaceHeld = true;
      event.preventDefault();
      return;
    }
    if (isEditing()) return;
    if (
      event.key === "Tab" &&
      !event.shiftKey &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey &&
      !editingGroup
    ) {
      const groupNode = nodes.find(
        (node) =>
          selected.has(node.id) && nodeTypeId(node) === "audio.group",
      );
      if (groupNode) {
        event.preventDefault();
        openVoiceGroup(groupNode);
        return;
      }
      const audioGroupNode = nodes.find(
        (node) =>
          selected.has(node.id) &&
          nodeTypeId(node) === "audio.group.instance" &&
          Boolean(node.group),
      );
      if (audioGroupNode && onOpenAudioGroup) {
        event.preventDefault();
        onOpenAudioGroup(audioGroupNode);
        return;
      }
      if (canExitGroup && onExitGroup) {
        event.preventDefault();
        onExitGroup();
        return;
      }
    }
    if (
      event.key === "Tab" &&
      event.ctrlKey &&
      !event.shiftKey &&
      !event.altKey &&
      onExitGroup
    ) {
      event.preventDefault();
      onExitGroup();
      return;
    }
    if (event.ctrlKey && event.key.toLowerCase() === "p" && track) {
      event.preventDefault();
      createFrame();
      return;
    }
    if (event.altKey && event.key.toLowerCase() === "p" && track) {
      event.preventDefault();
      const assignments: Record<string, string | null> = {};
      for (const node of nodes) {
        if (selected.has(node.id)) assignments[node.id] = null;
      }
      saveFrames(graph?.frames ?? {}, assignments);
      return;
    }
    if (
      event.key.toLowerCase() === "c" &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey
    ) {
      event.preventDefault();
      circleMode = !circleMode;
      circleDragging = false;
      if (circleMode) {
        circlePos =
          lastPointer.x !== 0 || lastPointer.y !== 0
            ? { ...lastPointer }
            : {
                x: (board?.clientWidth ?? 800) / 2,
                y: (board?.clientHeight ?? 500) / 2,
              };
      } else {
        circlePos = null;
      }
      return;
    }
    if (
      event.key.toLowerCase() === "h" &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey
    ) {
      event.preventDefault();
      toggleNodeFlag("collapsed");
      return;
    }
    if (
      event.key.toLowerCase() === "m" &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey
    ) {
      event.preventDefault();
      toggleNodeFlag("muted");
      return;
    }
    if (
      (event.ctrlKey || event.metaKey) &&
      event.altKey &&
      event.key.toLowerCase() === "g"
    ) {
      event.preventDefault();
      void ungroupAudioGroup();
      return;
    }
    if (
      (event.ctrlKey || event.metaKey) &&
      !event.shiftKey &&
      !event.altKey &&
      event.key.toLowerCase() === "g"
    ) {
      event.preventDefault();
      void makeAudioGroup();
      return;
    }
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
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "c") {
      event.preventDefault();
      copySelected();
      return;
    }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "v") {
      event.preventDefault();
      void pasteClipboard();
      return;
    }
    if (
      event.key.toLowerCase() === "n" &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey
    ) {
      event.preventDefault();
      sidebarOpen = !sidebarOpen;
      return;
    }
    if (
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey &&
      !event.shiftKey &&
      event.key.toLowerCase() === "a"
    ) {
      event.preventDefault();
      selected = new Set(nodes.map((node) => node.id));
      return;
    }
    if (event.altKey && event.key.toLowerCase() === "a") {
      event.preventDefault();
      selected = new Set();
      return;
    }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "i") {
      event.preventDefault();
      selected = new Set(
        nodes.filter((node) => !selected.has(node.id)).map((node) => node.id),
      );
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
      circleMode = false;
      circleDragging = false;
      circlePos = null;
      addMenu = null;
      nodeMenu = null;
      canvasMenu = null;
    }
  }

  function handleKeyUp(event: KeyboardEvent) {
    if (event.code === "Space" || event.key === " ") spaceHeld = false;
  }

  async function addNode(kind: string, at?: Point, group?: string) {
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
    const pending = pendingWire;
    pendingWire = null;
    addMenu = null;
    const nodeId = await onAddNode(track.id, kind, world.x, world.y, group);
    if (!nodeId || !pending) return;
    const info = audioTypeById.get(kind);
    if (!info) return;
    if (pending.side === "out") {
      const type = socketType(pending.node, "out", pending.port);
      const input = info.inputs.find((candidate) => candidate.type === type);
      if (input) {
        onConnect(track.id, pending.node, pending.port, nodeId, input.id);
      }
    } else {
      const type = socketType(pending.node, "in", pending.port);
      const output = info.outputs.find((candidate) => candidate.type === type);
      if (output) {
        onConnect(
          track.id,
          nodeId,
          output.id,
          pending.node,
          pending.port,
        );
      }
    }
  }
</script>

<svelte:window
  onmousemove={handleMouseMove}
  onmouseup={handleMouseUp}
  onkeydown={handleKeyDown}
  onkeyup={handleKeyUp}
/>

<section class="section graph-section" class:compact>
  <div class="section-head graph-head">
    <span class="index">08</span>
    <span class="title">{t("graph.title")}</span>
    <span class="rule"></span>
    <span class="meta">
      {#if editingGroup}
        {editingGroup.name} · {Object.keys(editingGroup.graph.nodes).length}
        {t("graph.nodes")}
      {:else if track}
        {track.id} · {nodes.length} {t("graph.nodes")}
      {:else}
        {t("graph.noTrack")}
      {/if}
    </span>
    {#if !editingGroup}
      <button
        class="ghost small"
        title={t("graph.sidebar")}
        onclick={() => (sidebarOpen = !sidebarOpen)}
      >N</button>
    {/if}
  </div>

  {#if track}
    {@const owner = track}
    {#if editingGroup}
      {#key editingGroup.id}
        <VoiceGraphEditor
          embedded
          groupId={editingGroup.id}
          groups={session.nodeGroups}
          nodeTypes={session.nodeTypes}
          path={voicePath}
          interface={editingGroup.interface}
          voice={editingGroup.graph}
          onEnterGroup={enterVoiceGroup}
          onNavigate={navigateVoice}
          onClose={() => (voicePath = [])}
        />
      {/key}
    {:else}
    <div class="graph-main">
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div
      class="graph-board"
      class:panning={Boolean(panDrag)}
      bind:this={board}
      role="application"
      aria-label={t("graph.title")}
      onmousedown={boardMouseDown}
      oncontextmenu={openCanvasMenu}
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
        {#each frames as frame (frame.id)}
          <div
            class="voice-frame"
            style:left={`${frame.position[0]}px`}
            style:top={`${frame.position[1]}px`}
            style:width={`${frame.size[0]}px`}
            style:height={`${frame.size[1]}px`}
          >
            <div
              class="voice-frame-head"
              role="button"
              tabindex="0"
              aria-label={frame.name}
              onmousedown={(event) => frameMouseDown(event, frame)}
              oncontextmenu={(event) => {
                event.preventDefault();
                event.stopPropagation();
                deleteFrame(frame);
              }}
            >
              {frame.name}
            </div>
            <div
              class="voice-frame-resize"
              role="button"
              tabindex="0"
              aria-label={t("voice.resizeFrame")}
              onmousedown={(event) => frameResizeDown(event, frame)}
            ></div>
          </div>
        {/each}
        <svg
          class="graph-wires"
          width={canvasSize.width}
          height={canvasSize.height}
          aria-hidden="true"
        >
          {#each connections as connection, index (`${connection.from.node}:${connection.from.port}->${connection.to.node}:${connection.to.port}:${index}`)}
            {#if insertTarget === connection}
              <path class="graph-wire-insert" d={wirePath(connection)} />
            {/if}
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
            <path
              class="graph-wire"
              d={wirePath(connection)}
              style:stroke={wireColor(connection)}
            />
          {/each}
          {#if wireDrag}
            <path
              class="graph-wire temp"
              d={tempWirePath()}
              style:stroke={wireDragColor()}
            />
          {/if}
        </svg>

        {#if marquee}
          <div class="graph-marquee" style={marqueeStyle()}></div>
        {/if}
        {#if lasso}
          <svg class="graph-lasso" aria-hidden="true">
            <polyline
              points={lasso.points
                .map((point) => `${point.x},${point.y}`)
                .join(" ")}
            />
          </svg>
        {/if}
        {#if cutLine}
          <svg class="graph-cut" aria-hidden="true">
            <polyline
              points={cutLine
                .map((point) => `${point.x},${point.y}`)
                .join(" ")}
            />
          </svg>
        {/if}
        {#if circleMode && circlePos}
          <div
            class="node-circle-select"
            style:left={`${circlePos.x - circleRadius}px`}
            style:top={`${circlePos.y - circleRadius}px`}
            style:width={`${circleRadius * 2}px`}
            style:height={`${circleRadius * 2}px`}
          ></div>
        {/if}

        {#each nodes as node (node.id)}
          {@const position = nodePositions[node.id]}
          {@const device = isDevice(node)}
          {@const isSelected = selected.has(node.id)}
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
          <div
            class="graph-node"
            class:device
            class:instrument={nodeTypeId(node).startsWith("audio.instrument")}
            class:output={nodeTypeId(node) === "audio.output"}
            class:selected={isSelected}
            class:collapsed={node.collapsed}
            class:muted={node.muted}
            class:wire-target={wireHover?.node === node.id}
            role="group"
            aria-label={nodeLabel(node)}
            data-node-id={node.id}
            data-node-kind={nodeTypeId(node)}
            data-category={nodeType(node)?.category ?? ""}
            style:left={`${position?.x ?? 0}px`}
            style:top={`${position?.y ?? 0}px`}
            style:height={`${position?.h ?? nodeHeight(node)}px`}
            style:width={`${position?.w ?? CARD_WIDTH}px`}
            onmousedown={(event) => nodeMouseDown(event, node)}
            oncontextmenu={(event) => openNodeMenu(event, node)}
          >
            <div
              class="graph-node-head"
              role="button"
              tabindex="0"
              aria-label={nodeLabel(node)}
              ondblclick={() => {
                if (nodeTypeId(node) === "audio.group") openVoiceGroup(node);
                else if (
                  nodeTypeId(node) === "audio.group.instance" &&
                  onOpenAudioGroup
                )
                  onOpenAudioGroup(node);
              }}
            >
              <span class="graph-node-kind">{nodeLabel(node)}</span>
              {#if node.muted}<span class="voice-node-muted">M</span>{/if}
              <span class="graph-node-id">{node.id}</span>
            </div>

            {#if !node.collapsed}
            {#if nodeTypeId(node) === "audio.group" || nodeTypeId(node) === "audio.group.instance"}
              <div class="graph-node-group-line">
                <code>{node.group ?? "—"}</code>
                {#if nodeTypeId(node) === "audio.group"}
                  <button
                    class="graph-node-enter"
                    title={t("graph.editGroup")}
                    disabled={!groupFor(node)}
                    onclick={() => openVoiceGroup(node)}
                  >
                    ↗
                  </button>
                {:else if onOpenAudioGroup}
                  <button
                    class="graph-node-enter"
                    title={t("graph.editGroup")}
                    onclick={() => onOpenAudioGroup?.(node)}
                  >
                    ↗
                  </button>
                {/if}
              </div>
            {/if}

            {#each nodeInputs(node) as input, index (input.id)}
              {@const param = matchingParam(node, input.id)}
              {@const linked = isLinked(node, input.id)}
              <div
                class="graph-socket-row in"
                style:top={`${portTop(node, index)}px`}
              >
                <button
                  type="button"
                  class="graph-port graph-port-in"
                  class:port-float={input.type === "float"}
                  class:port-gate={input.type === "gate"}
                  class:port-note={input.type === "note"}
                  class:port-audio={input.type === "audio"}
                  class:active={wireHover?.node === node.id &&
                    wireHover?.port === input.id}
                  style:top="7px"
                  aria-label={t("graph.inputPort")}
                  title={nodeTypeId(node) === "audio.group" &&
                  input.type === "note"
                    ? t("graph.noteInputHint")
                    : undefined}
                  data-input-node={node.id}
                  data-input-port={input.id}
                  onmousedown={(event) =>
                    startWire(event, node, "in", input.id)}
                ></button>
                <span class="graph-socket-name">{socketLabel(input.name)}</span>
                {#if input.panel}
                  <span class="graph-socket-panel">{input.panel}</span>
                {/if}
                {#if param && !linked}
                  <div class="graph-socket-value">
                    <NumberField
                      value={paramOf(node, param.name, param.default)}
                      min={param.minimum}
                      max={param.maximum}
                      step={param.step ?? 0.01}
                      precision={2}
                      disabled={busy}
                      onchange={(value) =>
                        onParamChange(owner.id, node.id, param.name, value)}
                    />
                  </div>
                {/if}
              </div>
            {/each}

            {#each nodeOutputs(node) as output, index (output.id)}
              <div
                class="graph-socket-row out"
                style:top={`${portTop(node, index)}px`}
              >
                <span class="graph-socket-name">{socketLabel(output.name)}</span>
                {#if output.panel}
                  <span class="graph-socket-panel">{output.panel}</span>
                {/if}
                <button
                  type="button"
                  class="graph-port graph-port-out"
                  class:port-float={output.type === "float"}
                  class:port-gate={output.type === "gate"}
                  class:port-note={output.type === "note"}
                  class:port-audio={output.type === "audio"}
                  class:active={wireHover?.node === node.id &&
                    wireDrag?.side === "in"}
                  style:top="7px"
                  aria-label={t("graph.startWire")}
                  title={t("graph.startWire")}
                  data-output-node={node.id}
                  onmousedown={(event) =>
                    startWire(event, node, "out", output.id)}
                ></button>
              </div>
            {/each}

            {#each extraParams(node) as param, index (param.name)}
              {@const top =
                NODE_HEADER +
                groupHeaderHeight(node) +
                NODE_BODY_TOP +
                (Math.max(nodeInputs(node).length, nodeOutputs(node).length) +
                  index) *
                  NODE_ROW}
              <div class="graph-socket-row param" style:top={`${top}px`}>
                <span class="graph-socket-name">{paramLabel(param.name)}</span>
                <div class="graph-socket-value">
                  <NumberField
                    value={paramOf(node, param.name, param.default)}
                    min={param.minimum}
                    max={param.maximum}
                    step={param.step ?? 0.01}
                    precision={2}
                    disabled={busy}
                    onchange={(value) =>
                      onParamChange(owner.id, node.id, param.name, value)}
                  />
                </div>
              </div>
            {/each}

            {#if node.plugin}
              {@const top =
                NODE_HEADER +
                groupHeaderHeight(node) +
                NODE_BODY_TOP +
                (Math.max(nodeInputs(node).length, nodeOutputs(node).length) +
                  extraParams(node).length) *
                  NODE_ROW}
              <div class="graph-socket-row param" style:top={`${top}px`}>
                <span class="graph-socket-name">{t("graph.plugin")}</span>
                <strong>{node.plugin.id}</strong>
              </div>
            {/if}
            {:else}
              {#each nodeInputs(node) as input, index (input.id)}
                <button
                  type="button"
                  class="graph-port graph-port-in"
                  class:port-float={input.type === "float"}
                  class:port-gate={input.type === "gate"}
                  class:port-note={input.type === "note"}
                  class:port-audio={input.type === "audio"}
                  style:top={`${portTop(node, index)}px`}
                  aria-label={t("graph.inputPort")}
                  data-input-node={node.id}
                  data-input-port={input.id}
                  onmousedown={(event) =>
                    startWire(event, node, "in", input.id)}
                ></button>
              {/each}
              {#each nodeOutputs(node) as output, index (output.id)}
                <button
                  type="button"
                  class="graph-port graph-port-out"
                  class:port-float={output.type === "float"}
                  class:port-gate={output.type === "gate"}
                  class:port-note={output.type === "note"}
                  class:port-audio={output.type === "audio"}
                  style:top={`${portTop(node, index)}px`}
                  aria-label={t("graph.startWire")}
                  data-output-node={node.id}
                  onmousedown={(event) =>
                    startWire(event, node, "out", output.id)}
                ></button>
              {/each}
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
          {#if nodeMenu && graph?.nodes[nodeMenu.nodeId]?.type_id === "audio.group"}
            <button
              role="menuitem"
              disabled={busy}
              onclick={() => {
                const node = nodeMenu
                  ? graph?.nodes[nodeMenu.nodeId]
                  : undefined;
                nodeMenu = null;
                if (node) openVoiceGroup(node);
              }}
            >
              {t("graph.editGroup")}
            </button>
          {/if}
          {#if onMakeAudioGroup}
            <button
              role="menuitem"
              disabled={busy}
              onclick={() => {
                nodeMenu = null;
                void makeAudioGroup();
              }}
            >
              {t("graph.packAudioGroup")}
            </button>
          {/if}
          {#if onUngroupAudioGroup && nodeMenu && graph?.nodes[nodeMenu.nodeId]?.type_id === "audio.group.instance"}
            <button
              role="menuitem"
              disabled={busy}
              onclick={() => {
                nodeMenu = null;
                void ungroupAudioGroup();
              }}
            >
              {t("graph.ungroupAudioGroup")}
            </button>
          {/if}
          <button role="menuitem" disabled={busy} onclick={duplicateFromMenu}>
            {t("graph.duplicate")}
          </button>
          <button
            role="menuitem"
            disabled={busy}
            onclick={() => {
              toggleNodeFlag("collapsed");
              nodeMenu = null;
            }}
          >
            {t("graph.collapse")}
          </button>
          <button
            role="menuitem"
            disabled={busy}
            onclick={() => {
              toggleNodeFlag("muted");
              nodeMenu = null;
            }}
          >
            {t("graph.mute")}
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

      {#if canvasMenu}
        <div
          class="graph-context-menu"
          style:left={`${canvasMenu.x}px`}
          style:top={`${canvasMenu.y}px`}
          role="menu"
          tabindex="-1"
          onmousedown={(event) => event.stopPropagation()}
        >
          <button
            role="menuitem"
            onclick={() => {
              addMenu = { x: canvasMenu?.x ?? 0, y: canvasMenu?.y ?? 0 };
              canvasMenu = null;
            }}
          >
            {t("graph.addNode")}
          </button>
          <button role="menuitem" onclick={() => { frameAll(); canvasMenu = null; }}>
            {t("graph.frameAll")}
          </button>
          <button role="menuitem" onclick={() => { frameSelected(); canvasMenu = null; }}>
            {t("graph.frameSelected")}
          </button>
          <button
            role="menuitem"
            onclick={() => {
              selected = new Set(nodes.map((node) => node.id));
              canvasMenu = null;
            }}
          >
            {t("graph.selectAll")}
          </button>
          <button
            role="menuitem"
            class="danger"
            disabled={selected.size === 0}
            onclick={() => {
              deleteSelected();
              canvasMenu = null;
            }}
          >
            {t("graph.delete")}
          </button>
        </div>
      {/if}

      {#if addMenu}
        <NodeAddMenu
          x={addMenu.x}
          y={addMenu.y}
          nodeTypes={addNodeTypes}
          groups={Object.values(session.nodeGroups)}
          groupLabel={t("voice.groupNode")}
          audioGroups={Object.values(session.audioGroups)}
          audioGroupLabel={t("graph.audioGroups")}
          onPickType={(typeId) => void addNode(typeId)}
          onPickGroup={(groupId) =>
            void addNode("audio.group", undefined, groupId)}
          onPickAudioGroup={(groupId) =>
            void addNode("audio.group.instance", undefined, groupId)}
          onClose={() => {
            addMenu = null;
            pendingWire = null;
          }}
        />
      {/if}
    </div>
    {#if sidebarOpen}
      <aside class="node-sidebar" aria-label={t("graph.sidebar")}>
        <div class="node-sidebar-tabs">
          <span class="node-sidebar-tab">{t("graph.nodeTab")}</span>
          <button
            class="node-sidebar-close"
            title={t("voice.close")}
            onclick={() => (sidebarOpen = false)}
          >×</button>
        </div>
        {#if graph?.nodes[active]}
          {@const node = graph.nodes[active]!}
          <div class="node-sidebar-section">
            <div class="node-sidebar-label">{t("graph.activeNode")}</div>
            <strong>{nodeLabel(node)}</strong>
            <code>{node.id}</code>
            <div class="node-sidebar-type">{nodeTypeId(node)}</div>
            {#if nodeTypeId(node) === "audio.group"}
              <div class="node-sidebar-group">
                {groupFor(node)?.name ?? node.group ?? "—"}
              </div>
              <button
                class="ghost small"
                disabled={!groupFor(node)}
                onclick={() => openVoiceGroup(node)}
              >
                {t("graph.editGroup")}
              </button>
            {/if}
            {#each sidebarSections(node) as [panel, params] (panel)}
              {#if panel}
                <div class="node-sidebar-panel">{panel}</div>
              {/if}
              {#each params as param (param.name)}
                <label class="node-sidebar-param">
                  <span>{paramLabel(param.name)}</span>
                  <NumberField
                    value={paramOf(node, param.name, param.default)}
                    min={param.minimum}
                    max={param.maximum}
                    step={param.step ?? 0.01}
                    precision={2}
                    disabled={busy}
                    onchange={(value) =>
                      onParamChange(owner.id, node.id, param.name, value)}
                  />
                </label>
              {/each}
            {/each}
          </div>
        {:else}
          <p class="node-sidebar-empty">{t("graph.noActiveNode")}</p>
        {/if}
      </aside>
    {/if}
    </div>
    {/if}

    <div class="section-foot">
      <span>{wireError || t("graph.foot")}</span>
      <span>{t("graph.direction")}</span>
    </div>
  {:else}
    <p class="empty">{t("graph.noTrack")}</p>
  {/if}
</section>

