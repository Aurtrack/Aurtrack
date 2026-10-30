<script lang="ts">
  import { onDestroy, tick } from "svelte";
  import type {
    NodeGroup,
    NodeGroupInterface,
    NodeTypeInfo,
    NodeTypeSocket,
    VoiceConnection,
    VoiceFrame,
    VoiceGraph,
    VoiceNode,
  } from "../api";
  import { errorMessage } from "../api";
  import { t, tLoose } from "../i18n.svelte";
  import {
    nodeTypeLabel,
    paramLabel,
    socketLabel,
  } from "../nodeLabels";
  import { session } from "../session.svelte";
  import NodeAddMenu from "./NodeAddMenu.svelte";
  import NumberField from "./NumberField.svelte";
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
    groupId: string;
    groups: Record<string, NodeGroup>;
    nodeTypes: NodeTypeInfo[];
    path: { id: string; name: string }[];
    interface: NodeGroupInterface;
    voice: VoiceGraph;
    embedded?: boolean;
    onEnterGroup: (groupId: string) => void;
    onNavigate: (index: number) => void;
    onClose: () => void;
  }

  let {
    groupId,
    groups,
    nodeTypes,
    path,
    interface: groupInterface,
    voice: initialVoice,
    embedded = false,
    onEnterGroup,
    onNavigate,
    onClose,
  }: Props = $props();

  const CANVAS_WIDTH = 1400;
  const CANVAS_HEIGHT = 620;

  type Point = { x: number; y: number };
  type Drag = {
    ids: string[];
    startX: number;
    startY: number;
    origins: Record<string, Point>;
    moved: boolean;
  };
  type Wire = {
    node: string;
    port: string;
    side: "in" | "out";
    ty: PortType;
    x: number;
    y: number;
  };

  type PortType = "float" | "int" | "gate" | "audio" | "note";
  const socketColors: Record<PortType, string> = {
    float: "#a1a1a1",
    int: "#7fbf7f",
    gate: "#cca6d6",
    audio: "#63c763",
    note: "#d99a4e",
  };
  const ZONE_SIDES = ["inputs", "outputs"] as const;
  type PortInfo = {
    id: string;
    name: string;
    ty: PortType;
    default?: number;
    minimum?: number;
    maximum?: number;
    step?: number;
    panel?: string;
    multi?: boolean;
  };

  let typeById = $derived(
    new Map(nodeTypes.map((nodeType) => [nodeType.id, nodeType])),
  );
  let menuTypes = $derived(
    nodeTypes.filter(
      (nodeType) =>
        nodeType.id !== "group.input" &&
        nodeType.id !== "group.output" &&
        nodeType.id !== "voice.group" &&
        nodeType.id !== "voice.repeat" &&
        nodeType.id !== "voice.zone.input" &&
        nodeType.id !== "voice.zone.output",
    ),
  );
  let swapTypes = $derived(
    menuTypes.filter((nodeType) => nodeType.id !== "voice.zone.repeat"),
  );

  function nodeGroup(node: VoiceNode): NodeGroup | undefined {
    return node.group ? groups[node.group] : undefined;
  }

  function isGroupNode(node: VoiceNode | undefined): boolean {
    return node?.type_id === "voice.group" || node?.type_id === "voice.repeat";
  }

  function isZoneChrome(node: VoiceNode): boolean {
    return node.type_id === "voice.zone.repeat";
  }

  function zoneHeaders(): VoiceNode[] {
    return Object.values(voice.nodes).filter(
      (node) => node.type_id === "voice.zone.repeat",
    );
  }

  function zoneBoundary(
    header: VoiceNode,
    typeId: "voice.zone.input" | "voice.zone.output",
  ): VoiceNode | undefined {
    return Object.values(voice.nodes).find(
      (node) => node.zone === header.id && node.type_id === typeId,
    );
  }

  function zonePortTop(
    node: VoiceNode,
    portId: string,
    side: "in" | "out",
  ): number | null {
    const headerId =
      node.type_id === "voice.zone.repeat" ? node.id : node.zone;
    if (!headerId) return null;
    const header = voice.nodes[headerId];
    if (!header) return null;
    if (node.type_id === "voice.zone.repeat") {
      if (side === "in") {
        if (portId === "count") return 38;
        const index = zoneSocketPorts(header.zone_inputs, {
          id: "out",
          name: "Audio",
        }).findIndex((port) => port.id === portId);
        return index < 0 ? null : 58 + index * 14;
      }
      const index = zoneSocketPorts(header.zone_outputs, {
        id: "in",
        name: "Audio",
      }).findIndex((port) => port.id === portId);
      return index < 0 ? null : 38 + index * 14;
    }
    if (node.type_id === "voice.zone.input") {
      const inputs = zoneSocketPorts(header.zone_inputs, {
        id: "out",
        name: "Audio",
      });
      const index = inputs.findIndex((port) => port.id === portId);
      return index < 0 ? null : 58 + index * 14;
    }
    if (node.type_id === "voice.zone.output") {
      const outputs = zoneSocketPorts(header.zone_outputs, {
        id: "in",
        name: "Audio",
      });
      const index = outputs.findIndex((port) => port.id === portId);
      return index < 0 ? null : 38 + index * 14;
    }
    return null;
  }

  function editableZoneSockets(
    header: VoiceNode,
    side: "inputs" | "outputs",
  ): NodeTypeSocket[] {
    const list = side === "inputs" ? header.zone_inputs : header.zone_outputs;
    if (list?.length) return list;
    return [
      {
        id: side === "inputs" ? "out" : "in",
        name: "Audio",
        type: "audio",
      },
    ];
  }

  function ensureZoneSockets(
    header: VoiceNode,
    side: "inputs" | "outputs",
  ): NodeTypeSocket[] {
    const list = editableZoneSockets(header, side);
    if (side === "inputs") header.zone_inputs = list;
    else header.zone_outputs = list;
    return list;
  }

  function syncZoneInterface(header: VoiceNode): void {
    const input = zoneBoundary(header, "voice.zone.input");
    const output = zoneBoundary(header, "voice.zone.output");
    if (input) input.zone_inputs = header.zone_inputs ?? [];
    if (output) output.zone_outputs = header.zone_outputs ?? [];
    touchVoice();
  }

  function addZoneSocket(header: VoiceNode, side: "inputs" | "outputs"): void {
    const list = ensureZoneSockets(header, side);
    let number = list.length + 1;
    const prefix = side === "inputs" ? "in" : "out";
    while (list.some((socket) => socket.id === `${prefix}-${number}`)) {
      number += 1;
    }
    list.push({
      id: `${prefix}-${number}`,
      name: side === "inputs" ? `Input ${number}` : `Output ${number}`,
      type: "audio",
    });
    if (side === "inputs") header.zone_inputs = [...list];
    else header.zone_outputs = [...list];
    syncZoneInterface(header);
  }

  function removeZoneSocket(
    header: VoiceNode,
    side: "inputs" | "outputs",
    index: number,
  ): void {
    const list = ensureZoneSockets(header, side);
    if (list.length <= 1) return;
    const [removed] = list.splice(index, 1);
    if (side === "inputs") header.zone_inputs = [...list];
    else header.zone_outputs = [...list];
    const boundary = zoneBoundary(
      header,
      side === "inputs" ? "voice.zone.input" : "voice.zone.output",
    );
    if (removed && boundary) {
      voice.connections = voice.connections.filter(
        (connection) =>
          !(
            connection.to.node === boundary.id &&
            connection.to.port === removed.id
          ) &&
          !(
            connection.from.node === boundary.id &&
            connection.from.port === removed.id
          ),
      );
    }
    syncZoneInterface(header);
  }

  function updateZoneSocket(
    header: VoiceNode,
    side: "inputs" | "outputs",
    index: number,
    patch: Partial<NodeTypeSocket>,
  ): void {
    const list = ensureZoneSockets(header, side);
    const socket = list[index];
    if (!socket) return;
    Object.assign(socket, patch);
    if (side === "inputs") header.zone_inputs = [...list];
    else header.zone_outputs = [...list];
    syncZoneInterface(header);
  }

  function startZoneSocketDrag(
    event: DragEvent,
    header: VoiceNode,
    side: "inputs" | "outputs",
    index: number,
  ): void {
    zoneSocketDrag = { headerId: header.id, side, index };
    event.dataTransfer?.setData("text/plain", `${header.id}:${side}:${index}`);
    if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
  }

  function dropZoneSocket(
    event: DragEvent,
    header: VoiceNode,
    side: "inputs" | "outputs",
    index: number,
  ): void {
    event.preventDefault();
    const drag = zoneSocketDrag;
    zoneSocketDrag = null;
    if (!drag || drag.headerId !== header.id || drag.side !== side) return;
    if (drag.index === index) return;
    const list = ensureZoneSockets(header, side);
    const [socket] = list.splice(drag.index, 1);
    if (!socket) return;
    list.splice(index, 0, socket);
    if (side === "inputs") header.zone_inputs = [...list];
    else header.zone_outputs = [...list];
    syncZoneInterface(header);
  }

  function zoneBoundaryPortY(node: VoiceNode, index: number): number {
    const offset = node.type_id === "voice.zone.input" ? 1 : 0;
    return (
      NODE_HEADER +
      NODE_BODY_TOP +
      (index + offset) * NODE_ROW +
      NODE_ROW / 2
    );
  }

  function zoneHeaderProxyPoint(
    header: VoiceNode,
    portId: string,
    side: "in" | "out",
  ): Point | null {
    if (side === "in") {
      const inputNode = zoneBoundary(header, "voice.zone.input");
      if (!inputNode) return null;
      const sockets = zoneSocketPorts(header.zone_inputs, {
        id: "out",
        name: "Audio",
      });
      const field = sockets.findIndex((port) => port.id === portId);
      if (portId !== "count" && field < 0) return null;
      const index = portId === "count" ? 0 : field + 1;
      return {
        x: inputNode.position?.[0] ?? 0,
        y: (inputNode.position?.[1] ?? 0) + zoneHeaderProxyY(index),
      };
    }
    const outputNode = zoneBoundary(header, "voice.zone.output");
    if (!outputNode) return null;
    const sockets = zoneSocketPorts(header.zone_outputs, {
      id: "in",
      name: "Audio",
    });
    const index = sockets.findIndex((port) => port.id === portId);
    if (index < 0) return null;
    return {
      x: (outputNode.position?.[0] ?? 0) + nodeWidth(outputNode),
      y: (outputNode.position?.[1] ?? 0) + zoneHeaderProxyY(index),
    };
  }

  function zoneHeaderProxyY(index: number): number {
    return NODE_HEADER + NODE_BODY_TOP + index * NODE_ROW + NODE_ROW / 2;
  }

  function zonePortPoint(
    node: VoiceNode,
    portId: string,
    side: "in" | "out",
  ): Point | null {
    if (node.type_id === "voice.zone.repeat") {
      return zoneHeaderProxyPoint(node, portId, side);
    }
    if (node.type_id === "voice.zone.input" || node.type_id === "voice.zone.output") {
      const outputs = outputList(node);
      const inputs = inputList(node);
      const index =
        side === "out"
          ? outputs.findIndex((port) => port.id === portId)
          : inputs.findIndex((port) => port.id === portId);
      if (index < 0) return null;
      return {
        x:
          side === "out"
            ? (node.position?.[0] ?? 0) + nodeWidth(node)
            : (node.position?.[0] ?? 0),
        y: (node.position?.[1] ?? 0) + zoneBoundaryPortY(node, index),
      };
    }
    const headerId = node.zone;
    if (!headerId) return null;
    const header = voice.nodes[headerId];
    if (!header) return null;
    const top = zonePortTop(node, portId, side);
    if (top === null) return null;
    const bounds = zoneBounds(header);
    const left = node.type_id === "voice.zone.input" || side === "in";
    return {
      x: left ? bounds.left + 5 : bounds.left + bounds.width - 5,
      y: bounds.top + top,
    };
  }

  function zoneMemberIds(zoneId: string, seen = new Set<string>()): string[] {
    const ids: string[] = [];
    for (const node of Object.values(voice.nodes)) {
      if (node.zone !== zoneId || seen.has(node.id)) continue;
      seen.add(node.id);
      ids.push(node.id);
      if (node.type_id === "voice.zone.repeat") {
        ids.push(...zoneMemberIds(node.id, seen));
      }
    }
    return ids;
  }

  function zoneBounds(
    header: VoiceNode,
    excludeId?: string,
  ): {
    left: number;
    top: number;
    width: number;
    height: number;
  } {
    if (header.collapsed) {
      const [x, y] = header.position ?? [0, 0];
      return {
        left: x - 24,
        top: y - 34,
        width: Math.max(320, header.width ?? 360),
        height: 96,
      };
    }
    if (!excludeId && header.width && header.height) {
      const [x, y] = header.position ?? [0, 0];
      return {
        left: x - 24,
        top: y - 34,
        width: Math.max(320, header.width),
        height: Math.max(160, header.height),
      };
    }
    const nodes = [
      header,
      ...zoneMemberIds(header.id)
        .filter((id) => id !== excludeId)
        .map((id) => voice.nodes[id])
        .filter((node): node is VoiceNode => node !== undefined),
    ].filter((node) => node.id !== excludeId);
    let minX = Infinity;
    let minY = Infinity;
    let maxX = -Infinity;
    let maxY = -Infinity;
    for (const node of nodes) {
      const [x, y] = node.position ?? [0, 0];
      minX = Math.min(minX, x);
      minY = Math.min(minY, y);
      maxX = Math.max(maxX, x + nodeWidth(node));
      maxY = Math.max(maxY, y + nodeHeight(node));
    }
    const pad = 24;
    return {
      left: minX - pad,
      top: minY - 34,
      width: Math.max(260, maxX - minX + pad * 2),
      height: Math.max(180, maxY - minY + 34 + pad),
    };
  }

  function expandZoneSelection(ids: Iterable<string>): Set<string> {
    const expanded = new Set(ids);
    for (const id of [...expanded]) {
      if (voice.nodes[id]?.type_id === "voice.zone.repeat") {
        for (const member of zoneMemberIds(id)) expanded.add(member);
      }
    }
    return expanded;
  }

  function isInCollapsedZone(node: VoiceNode): boolean {
    let zone = node.zone;
    while (zone) {
      const header = voice.nodes[zone];
      if (!header) return false;
      if (header.collapsed) return true;
      zone = header.zone;
    }
    return false;
  }

  function isCollapsedConnection(connection: VoiceConnection): boolean {
    const source = voice.nodes[connection.from.node];
    const target = voice.nodes[connection.to.node];
    return (
      (source !== undefined && isInCollapsedZone(source)) ||
      (target !== undefined && isInCollapsedZone(target))
    );
  }

  function toggleZoneCollapsed(header: VoiceNode): void {
    header.collapsed = !header.collapsed;
    touchVoice();
  }

  function zoneAtPoint(
    point: [number, number],
    excludeId?: string,
  ): string | undefined {
    for (const header of zoneHeaders()) {
      if (header.id === excludeId) continue;
      const bounds = zoneBounds(header, excludeId);
      if (
        point[0] >= bounds.left &&
        point[0] <= bounds.left + bounds.width &&
        point[1] >= bounds.top &&
        point[1] <= bounds.top + bounds.height
      ) {
        return header.id;
      }
    }
    return undefined;
  }

  function muteActionLabel(node: VoiceNode | undefined): string {
    const bypass = node
      ? inputList(node).some((port) => port.ty === "audio")
      : false;
    return t(bypass ? "graph.bypass" : "graph.mute");
  }

  function nodeLabel(node: VoiceNode): string {
    if (node.label?.trim()) return node.label.trim();
    if (node.type_id === "group.input") {
      return tLoose("node.group.input", "GROUP INPUT").toUpperCase();
    }
    if (node.type_id === "group.output") {
      return tLoose("node.group.output", "GROUP OUTPUT").toUpperCase();
    }
    if (
      node.type_id === "voice.repeat" ||
      node.type_id === "voice.zone.repeat"
    ) {
      return tLoose("node.voice.repeat", "REPEAT").toUpperCase();
    }
    if (
      node.type_id === "voice.zone.input" ||
      node.type_id === "voice.zone.output"
    ) {
      return tLoose("node.voice.repeat", "REPEAT").toUpperCase();
    }
    if (isGroupNode(node)) {
      return nodeGroup(node)?.name ?? node.group ?? tLoose("node.voice.group", "VOICE GROUP");
    }
    const info = typeById.get(node.type_id);
    return (info ? nodeTypeLabel(info) : node.type_id).toUpperCase();
  }

  function interfacePorts(sockets: NodeGroupInterface["inputs"]): PortInfo[] {
    return sockets.map((socket) => ({
      id: socket.id,
      name: socket.name,
      ty: socket.type,
      default: socket.default,
      minimum: socket.minimum,
      maximum: socket.maximum,
      step: socket.step,
      panel: socket.panel,
      multi: false,
    }));
  }

  function typePorts(
    node: VoiceNode,
    side: "inputs" | "outputs",
  ): PortInfo[] {
    return (typeById.get(node.type_id)?.[side] ?? []).map((socket) => ({
      id: socket.id,
      name: socket.name,
      ty: socket.type,
      default: socket.default,
      minimum: socket.minimum,
      maximum: socket.maximum,
      step: socket.step,
      multi: socket.multi,
    }));
  }

  function zoneSocketPorts(
    sockets: NodeTypeSocket[] | undefined,
    fallback: { id: string; name: string },
  ): PortInfo[] {
    const list = sockets?.length
      ? sockets
      : [
          {
            id: fallback.id,
            name: fallback.name,
            type: "audio" as const,
            default: 0,
          },
        ];
    return list.map((socket) => ({
      id: socket.id,
      name: socket.name,
      ty: socket.type,
      default: socket.default,
      minimum: socket.minimum,
      maximum: socket.maximum,
      step: socket.step,
    }));
  }

  function allInputList(node: VoiceNode): PortInfo[] {
    if (node.type_id === "voice.zone.repeat") {
      return [
        ...typePorts(node, "inputs").filter((port) => port.id === "count"),
        ...zoneSocketPorts(node.zone_inputs, { id: "out", name: "Audio" }),
      ];
    }
    if (node.type_id === "voice.zone.output") {
      return zoneSocketPorts(node.zone_outputs, { id: "in", name: "Audio" });
    }
    if (
      node.type_id === "voice.index_switch" ||
      node.type_id === "voice.index_switch_float"
    ) {
      const count = Math.max(
        1,
        Math.min(16, Math.round(node.params.count ?? 4)),
      );
      const ty: PortType =
        node.type_id === "voice.index_switch" ? "audio" : "float";
      return [
        {
          id: "index",
          name: "Index",
          ty: "float",
          default: 0,
          minimum: 0,
          maximum: count - 1,
          step: 1,
        },
        ...Array.from({ length: count }, (_, index) => ({
          id: String(index),
          name: String(index),
          ty,
          default: 0,
          minimum: -Infinity,
          maximum: Infinity,
          step: 0.01,
        })),
      ];
    }
    if (node.type_id === "group.input") return [];
    if (node.type_id === "group.output") {
      return interfacePorts(groupInterface.outputs);
    }
    if (node.type_id === "voice.group") {
      return interfacePorts(nodeGroup(node)?.interface.inputs ?? []);
    }
    return typePorts(node, "inputs");
  }

  function allOutputList(node: VoiceNode): PortInfo[] {
    if (node.type_id === "voice.zone.repeat") {
      return zoneSocketPorts(node.zone_outputs, { id: "in", name: "Audio" });
    }
    if (node.type_id === "voice.zone.input") {
      return zoneSocketPorts(node.zone_inputs, { id: "out", name: "Audio" });
    }
    if (node.type_id === "voice.zone.output") return [];
    if (node.type_id === "group.output") return [];
    if (node.type_id === "group.input") {
      return interfacePorts(groupInterface.inputs);
    }
    if (node.type_id === "voice.group") {
      return interfacePorts(nodeGroup(node)?.interface.outputs ?? []);
    }
    return typePorts(node, "outputs");
  }

  function inputList(node: VoiceNode): PortInfo[] {
    const ports = allInputList(node);
    return hideUnconnected
      ? ports.filter((port) => isLinked(node, port.id))
      : ports;
  }

  function outputList(node: VoiceNode): PortInfo[] {
    const ports = allOutputList(node);
    return hideUnconnected
      ? ports.filter((port) =>
          voice.connections.some(
            (connection) =>
              !connection.muted &&
              connection.from.node === node.id &&
              connection.from.port === port.id,
          ),
        )
      : ports;
  }

  // 编辑器只在打开时创建，初始快照之后由本地编辑状态接管。
  // svelte-ignore state_referenced_locally
  let voice = $state<VoiceGraph>(
    JSON.parse(JSON.stringify(initialVoice)) as VoiceGraph,
  );
  let loadedGroupId = $state("");

  $effect(() => {
    if (groupId === loadedGroupId) return;
    loadedGroupId = groupId;
    voice = JSON.parse(JSON.stringify(initialVoice)) as VoiceGraph;
    selected = new Set();
    active = "";
  });
  let interfaceOpen = $state(false);
  let sidebarOpen = $state(false);
  let groupNameDraft = $state("");
  $effect(() => {
    groupNameDraft = groups[groupId]?.name ?? "";
  });
  let interfaceDraft = $state<NodeGroupInterface>({
    inputs: [],
    outputs: [],
  });
  let interfaceError = $state("");
  let preview = $state<{
    values: number[];
    minimum: number;
    maximum: number;
  } | null>(null);
  let previewError = $state("");
  let previewLoading = $state(false);

  async function loadPreview(): Promise<void> {
    if (!active) return;
    previewLoading = true;
    previewError = "";
    try {
      preview = await session.previewVoice(groupId, active);
    } catch (cause) {
      previewError = errorMessage(cause);
      preview = null;
    } finally {
      previewLoading = false;
    }
  }

  function previewPoints(): string {
    const plot = preview;
    if (!plot || plot.values.length < 2) return "";
    const min = plot.minimum;
    const max = plot.maximum;
    const span = Math.max(1e-9, max - min);
    return plot.values
      .map((value, index) => {
        const x = (index / (plot.values.length - 1)) * 132;
        const y = 32 - ((value - min) / span) * 28;
        return `${x.toFixed(2)},${y.toFixed(2)}`;
      })
      .join(" ");
  }
  let libraryOpen = $state(false);
  let libraryError = $state("");
  let newGroupName = $state("");
  let autoSaveReady = false;
  let autoSaveTimer: ReturnType<typeof setTimeout> | null = null;
  const interfaceSides = $derived([
    { key: "inputs" as const, label: t("graph.inputs") },
    { key: "outputs" as const, label: t("graph.outputs") },
  ]);
  let selected = $state<Set<string>>(new Set());
  let active = $state("");
  let view = $state({ x: 0, y: 0, scale: 1 });
  let pan = $state<{
    startX: number;
    startY: number;
    viewX: number;
    viewY: number;
  } | null>(null);
  let marquee = $state<{
    x1: number;
    y1: number;
    x2: number;
    y2: number;
  } | null>(null);
  let lasso = $state<{ points: Point[]; subtract: boolean } | null>(null);
  let insertTarget = $state<VoiceConnection | null>(null);
  let cutLine = $state<Point[] | null>(null);
  let muteLine = $state<Point[] | null>(null);
  let hideUnconnected = $state(false);
  let circleMode = $state(false);
  let circleRadius = $state(48);
  let circlePos = $state<Point | null>(null);
  let circleDragging = false;
  let drag = $state<Drag | null>(null);
  let nodeResize = $state<{
    ids: string[];
    startX: number;
    widths: Record<string, number>;
  } | null>(null);
  let nodeMenu = $state<{ x: number; y: number; nodeId: string } | null>(null);
  let pendingLink = $state<Wire | null>(null);
  let movingWire = $state<VoiceConnection | null>(null);
  let wireMoved = false;
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
  let zoneSocketDrag = $state<{
    headerId: string;
    side: "inputs" | "outputs";
    index: number;
  } | null>(null);
  let zoneResize = $state<{
    id: string;
    startX: number;
    startY: number;
    width: number;
    height: number;
  } | null>(null);
  let wire = $state<Wire | null>(null);
  let error = $state("");
  let saving = $state(false);
  let addMenu = $state<{ x: number; y: number } | null>(null);
  let swapMenu = $state<{ x: number; y: number } | null>(null);
  let lastPointer = $state({ x: 0, y: 0 });
  let boardElement = $state<HTMLElement | null>(null);
  let hostElement = $state<HTMLElement | null>(null);
  let labelInput = $state<HTMLInputElement | null>(null);
  let editingFrameId = $state<string | null>(null);
  let frameNameInput = $state<HTMLInputElement | null>(null);
  let groupChoices = $derived(
    Object.values(groups).filter((group) => group.id !== groupId),
  );

  let nodes = $derived(Object.values(voice.nodes));
  let frames = $derived(Object.values(voice.frames ?? {}));
  let positions = $derived.by(() => {
    const result: Record<string, { x: number; y: number }> = {};
    nodes.forEach((node, index) => {
      result[node.id] = node.position
        ? { x: node.position[0], y: node.position[1] }
        : {
            x: 40 + (index % 3) * 260,
            y: 60 + Math.floor(index / 3) * 150,
          };
    });
    return result;
  });

  let wireBounds = $derived.by(() => {
    let minX = 0;
    let minY = 0;
    let maxX = CANVAS_WIDTH;
    let maxY = CANVAS_HEIGHT;
    for (const node of nodes) {
      const position = positions[node.id];
      if (!position) continue;
      minX = Math.min(minX, position.x);
      minY = Math.min(minY, position.y);
      maxX = Math.max(maxX, position.x + nodeWidth(node));
      maxY = Math.max(maxY, position.y + nodeHeight(node));
    }
    const padding = 240;
    return {
      x: minX - padding,
      y: minY - padding,
      width: maxX - minX + padding * 2,
      height: maxY - minY + padding * 2,
    };
  });

  function groupHeaderHeight(node: VoiceNode): number {
    return isGroupNode(node) ? 18 : 0;
  }

  function matchingParam(node: VoiceNode, portId: string) {
    const declared = typeById
      .get(node.type_id)
      ?.params.find((param) => param.name === portId);
    if (declared) return declared;
    const staticSocket = typeById
      .get(node.type_id)
      ?.inputs.find((socket) => socket.id === portId);
    if (
      staticSocket &&
      staticSocket.type !== "audio" &&
      staticSocket.type !== "note"
    ) {
      return {
        name: staticSocket.id,
        default: staticSocket.default ?? 0,
        minimum: staticSocket.minimum ?? -Infinity,
        maximum: staticSocket.maximum ?? Infinity,
        step: staticSocket.step ?? 0.01,
      };
    }
    if (node.type_id !== "voice.group") return undefined;
    const socket = nodeGroup(node)?.interface.inputs.find(
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
    };
  }

  function isLinked(node: VoiceNode, portId: string): boolean {
    return voice.connections.some(
      (connection) =>
        !connection.muted &&
        connection.to.node === node.id &&
        connection.to.port === portId,
    );
  }

  function inputIsMulti(node: VoiceNode, portId: string): boolean {
    return allInputList(node).find((port) => port.id === portId)?.multi === true;
  }

  function removeConnection(event: Event, connection: VoiceConnection): void {
    event.preventDefault();
    event.stopPropagation();
    voice.connections = voice.connections.filter(
      (candidate) => candidate !== connection,
    );
  }

  function extraParams(node: VoiceNode) {
    const ports = new Set(inputList(node).map((port) => port.id));
    return (typeById.get(node.type_id)?.params ?? []).filter(
      (param) => !ports.has(param.name),
    );
  }

  function rowCount(node: VoiceNode): number {
    return (
      Math.max(inputList(node).length, outputList(node).length) +
      extraParams(node).length
    );
  }

  function collapsedRows(node: VoiceNode): number {
    return Math.max(1, Math.max(inputList(node).length, outputList(node).length));
  }

  function portY(node: VoiceNode, index: number): number {
    if (node.collapsed) return collapsedPortY(index);
    return expandedPortY(index, groupHeaderHeight(node));
  }

  function portTop(node: VoiceNode, index: number): number {
    return node.collapsed
      ? portY(node, index)
      : portY(node, index) - NODE_ROW / 2;
  }

  function nodeHeight(node: VoiceNode): number {
    if (node.collapsed) return collapsedNodeHeight(collapsedRows(node));
    if (node.type_id === "voice.zone.input") {
      const header = node.zone ? voice.nodes[node.zone] : undefined;
      const count = header
        ? zoneSocketPorts(header.zone_inputs, { id: "out", name: "Audio" }).length
        : 0;
      return expandedNodeHeight(count + 1, 0);
    }
    if (node.type_id === "voice.zone.output") {
      const header = node.zone ? voice.nodes[node.zone] : undefined;
      const count = header
        ? zoneSocketPorts(header.zone_outputs, { id: "in", name: "Audio" }).length
        : 0;
      return expandedNodeHeight(count, 0);
    }
    return expandedNodeHeight(rowCount(node), groupHeaderHeight(node));
  }

  function nodeWidth(node: VoiceNode): number {
    return Math.max(140, Math.min(480, node.width ?? NODE_WIDTH));
  }

  function wireColor(connection: VoiceConnection): string {
    if (connection.muted) return "#b85a5a";
    const source = voice.nodes[connection.from.node];
    if (!source) return socketColors.audio;
    const port = outputList(source).find(
      (candidate) => candidate.id === connection.from.port,
    );
    return socketColors[port?.ty ?? "audio"];
  }

  function wirePath(connection: VoiceConnection): string {
    const from = positions[connection.from.node];
    const to = positions[connection.to.node];
    const target = voice.nodes[connection.to.node];
    if (!from || !to || !target) return "";
    const source = voice.nodes[connection.from.node];
    if (!source) return "";
    const outputs = outputList(source);
    const inputs = inputList(target);
    const outIndex = Math.max(
      0,
      outputs.findIndex((port) => port.id === connection.from.port),
    );
    const inIndex = Math.max(
      0,
      inputs.findIndex((port) => port.id === connection.to.port),
    );
    const start = zonePortPoint(source, connection.from.port, "out");
    const end = zonePortPoint(target, connection.to.port, "in");
    const x1 = start?.x ?? from.x + nodeWidth(source);
    const y1 = start?.y ?? from.y + portY(source, outIndex);
    const x2 = end?.x ?? to.x;
    const y2 = end?.y ?? to.y + portY(target, inIndex);
    const bend = wireBend(x1, x2);
    return `M ${x1} ${y1} C ${x1 + bend} ${y1}, ${x2 - bend} ${y2}, ${x2} ${y2}`;
  }

  function wireBend(x1: number, x2: number): number {
    const gap = x2 - x1;
    return gap > 0 ? Math.min(40, Math.max(2, gap * 0.5)) : 60;
  }

  function connectionEnds(
    connection: VoiceConnection,
  ): { from: Point; to: Point } | null {
    const source = voice.nodes[connection.from.node];
    const target = voice.nodes[connection.to.node];
    const sourcePosition = positions[connection.from.node];
    const targetPosition = positions[connection.to.node];
    if (!source || !target || !sourcePosition || !targetPosition) return null;
    const outIndex = Math.max(
      0,
      outputList(source).findIndex((port) => port.id === connection.from.port),
    );
    const inIndex = Math.max(
      0,
      inputList(target).findIndex((port) => port.id === connection.to.port),
    );
    return {
      from: zonePortPoint(source, connection.from.port, "out") ?? {
        x: sourcePosition.x + nodeWidth(source),
        y: sourcePosition.y + portY(source, outIndex),
      },
      to: zonePortPoint(target, connection.to.port, "in") ?? {
        x: targetPosition.x,
        y: targetPosition.y + portY(target, inIndex),
      },
    };
  }

  function wireSamples(
    connection: VoiceConnection,
    steps = 24,
  ): Point[] {
    const ends = connectionEnds(connection);
    if (!ends) return [];
    const bend = wireBend(ends.from.x, ends.to.x);
    const points: Point[] = [];
    for (let step = 0; step <= steps; step += 1) {
      const t = step / steps;
      const inverse = 1 - t;
      points.push({
        x:
          inverse ** 3 * ends.from.x +
          3 * inverse ** 2 * t * (ends.from.x + bend) +
          3 * inverse * t ** 2 * (ends.to.x - bend) +
          t ** 3 * ends.to.x,
        y:
          inverse ** 3 * ends.from.y +
          3 * inverse ** 2 * t * ends.from.y +
          3 * inverse * t ** 2 * ends.to.y +
          t ** 3 * ends.to.y,
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

  function cutWire(connection: VoiceConnection, line: Point[]): boolean {
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

  function findInsertTarget(node: VoiceNode): VoiceConnection | null {
    const position = positions[node.id];
    if (!position) return null;
    const center = {
      x: position.x + nodeWidth(node) / 2,
      y: position.y + nodeHeight(node) / 2,
    };
    const inputs = inputList(node);
    const outputs = outputList(node);
    if (inputs.length === 0 || outputs.length === 0) return null;
    for (const connection of voice.connections) {
      if (
        connection.from.node === node.id ||
        connection.to.node === node.id
      ) {
        continue;
      }
      const ends = connectionEnds(connection);
      if (!ends) continue;
      const bend = Math.max(40, Math.abs(ends.to.x - ends.from.x) * 0.45);
      let closest = Infinity;
      let previous = ends.from;
      for (let step = 1; step <= 18; step += 1) {
        const t = step / 18;
        const inverse = 1 - t;
        const point = {
          x:
            inverse ** 3 * ends.from.x +
            3 * inverse ** 2 * t * (ends.from.x + bend) +
            3 * inverse * t ** 2 * (ends.to.x - bend) +
            t ** 3 * ends.to.x,
          y:
            inverse ** 3 * ends.from.y +
            3 * inverse ** 2 * t * ends.from.y +
            3 * inverse * t ** 2 * ends.to.y +
            t ** 3 * ends.to.y,
        };
        closest = Math.min(
          closest,
          pointSegmentDistance(center, previous, point),
        );
        previous = point;
      }
      const source = voice.nodes[connection.from.node];
      const type = source
        ? outputList(source).find((port) => port.id === connection.from.port)
            ?.ty
        : undefined;
      if (
        closest <= 26 &&
        type &&
        inputs.some((port) => port.ty === type) &&
        outputs.some((port) => port.ty === type)
      ) {
        return connection;
      }
    }
    return null;
  }

  function insertNodeOnLink(node: VoiceNode, connection: VoiceConnection) {
    const source = voice.nodes[connection.from.node];
    const type = source
      ? outputList(source).find((port) => port.id === connection.from.port)?.ty
      : undefined;
    if (!type) return;
    const input = inputList(node).find((port) => port.ty === type);
    const output = outputList(node).find((port) => port.ty === type);
    if (!input || !output) return;
    voice.connections = voice.connections.filter(
      (candidate) => candidate !== connection,
    );
    voice.connections.push({
      from: { ...connection.from },
      to: { node: node.id, port: input.id },
    });
    voice.connections.push({
      from: { node: node.id, port: output.id },
      to: { ...connection.to },
    });
  }

  function tempWirePath(): string {
    const activeWire = wire;
    if (!activeWire || !boardElement) return "";
    const node = voice.nodes[activeWire.node];
    const position = node ? positions[activeWire.node] : undefined;
    if (!node || !position) return "";
    const ports =
      activeWire.side === "out" ? outputList(node) : inputList(node);
    const index = Math.max(
      0,
      ports.findIndex((candidate) => candidate.id === activeWire.port),
    );
    const x1 =
      activeWire.side === "out" ? position.x + nodeWidth(node) : position.x;
    const y1 = position.y + portY(node, index);
    const x2 = activeWire.x;
    const y2 = activeWire.y;
    if (activeWire.side === "in") {
      const bend = Math.max(40, Math.abs(x2 - x1) * 0.45);
      return `M ${x2} ${y2} C ${x2 - bend} ${y2}, ${x1 + bend} ${y1}, ${x1} ${y1}`;
    }
    const bend = Math.max(40, Math.abs(x2 - x1) * 0.45);
    return `M ${x1} ${y1} C ${x1 + bend} ${y1}, ${x2 - bend} ${y2}, ${x2} ${y2}`;
  }

  function boardPoint(event: { clientX: number; clientY: number }): Point | null {
    if (!boardElement) return null;
    const rect = boardElement.getBoundingClientRect();
    return { x: event.clientX - rect.left, y: event.clientY - rect.top };
  }

  function worldPoint(event: { clientX: number; clientY: number }): Point | null {
    const point = boardPoint(event);
    if (!point) return null;
    return {
      x: (point.x - view.x) / view.scale,
      y: (point.y - view.y) / view.scale,
    };
  }

  function selectOnly(id: string) {
    selected = new Set([id]);
    active = id;
  }

  function detachInputWire(
    existing: VoiceConnection,
    point: Point,
    remember: boolean,
  ): void {
    const source = voice.nodes[existing.from.node];
    const sourcePort = source
      ? outputList(source).find(
          (candidate) => candidate.id === existing.from.port,
        )
      : undefined;
    if (!source || !sourcePort) return;
    voice.connections = voice.connections.filter(
      (connection) => connection !== existing,
    );
    selectOnly(source.id);
    wireMoved = false;
    movingWire = remember ? { ...existing } : null;
    wire = {
      node: source.id,
      port: sourcePort.id,
      side: "out",
      ty: sourcePort.ty,
      x: point.x,
      y: point.y,
    };
    touchVoice();
  }

  function zoneSocketDown(
    event: MouseEvent,
    header: VoiceNode,
    kind: "input" | "output",
    port: PortInfo,
  ): void {
    if (circleMode) return;
    if (wire) {
      if (kind === "input") {
        if (wire.side === "out") finishWire(event, header, port);
        else {
          wire = null;
          wireMoved = false;
        }
        return;
      }
      const outputNode = zoneBoundary(header, "voice.zone.output");
      const target = outputNode
        ? allInputList(outputNode).find((candidate) => candidate.id === port.id)
        : undefined;
      if (outputNode && target && wire.side === "out") {
        finishWire(event, outputNode, target);
      } else {
        wire = null;
        wireMoved = false;
      }
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    if (kind === "input") {
      const inputNode = zoneBoundary(header, "voice.zone.input");
      const source = inputNode
        ? allOutputList(inputNode).find((candidate) => candidate.id === port.id)
        : undefined;
      if (inputNode && source) startWire(event, inputNode, source, "out");
      return;
    }
    startWire(event, header, port, "out");
  }

  function zoneSocketUp(
    event: MouseEvent,
    header: VoiceNode,
    kind: "input" | "output",
    port: PortInfo,
  ): void {
    if (!wire) return;
    const self =
      kind === "input"
        ? zoneBoundary(header, "voice.zone.input")?.id
        : header.id;
    if (wire.node === self) {
      wire = null;
      wireMoved = false;
      return;
    }
    zoneSocketDown(event, header, kind, port);
  }

  function startWire(
    event: MouseEvent,
    node: VoiceNode,
    port: PortInfo,
    side: "in" | "out",
  ) {
    if (circleMode) return;
    event.preventDefault();
    event.stopPropagation();
    const point = worldPoint(event);
    if (!point) return;
    if ((event.ctrlKey || event.altKey) && side === "in" && !wire) {
      const existing = voice.connections.find(
        (connection) =>
          !connection.muted &&
          connection.to.node === node.id &&
          connection.to.port === port.id,
      );
      if (existing) {
        detachInputWire(existing, point, event.altKey);
        return;
      }
    }
    if (wire) {
      if (wire.side !== side) finishWire(event, node, port);
      else wire = null;
      return;
    }
    pendingLink = null;
    selectOnly(node.id);
    wireMoved = false;
    wire = {
      node: node.id,
      port: port.id,
      side,
      ty: port.ty,
      x: point.x,
      y: point.y,
    };
  }

  function restoreMovingWire(): void {
    if (!movingWire) return;
    voice.connections.push(movingWire);
    movingWire = null;
    touchVoice();
  }

  function finishWire(
    event: MouseEvent,
    node: VoiceNode,
    port: PortInfo,
  ) {
    event.preventDefault();
    event.stopPropagation();
    const active = wire;
    if (!active) return;
    wire = null;
    if (active.node === node.id || port.ty !== active.ty) {
      if (port.ty !== active.ty) {
        error = `端口类型不匹配：${active.ty} → ${port.ty}`;
      }
      restoreMovingWire();
      return;
    }
    const from =
      active.side === "out"
        ? { node: active.node, port: active.port }
        : { node: node.id, port: port.id };
    const to =
      active.side === "out"
        ? { node: node.id, port: port.id }
        : { node: active.node, port: active.port };
    if (from.node === to.node) {
      restoreMovingWire();
      return;
    }
    const exists = voice.connections.some(
      (connection) =>
        !connection.muted &&
        connection.from.node === from.node &&
        connection.from.port === from.port &&
        connection.to.node === to.node &&
        connection.to.port === to.port,
    );
    if (exists) {
      restoreMovingWire();
      return;
    }
    if (port.multi) {
      voice.connections.push({ from, to });
      movingWire = null;
      touchVoice();
      return;
    }
    const displaced = voice.connections.filter(
      (connection) =>
        !connection.muted &&
        connection.to.node === to.node &&
        connection.to.port === to.port &&
        !(
          connection.from.node === from.node &&
          connection.from.port === from.port
        ),
    );
    voice.connections = voice.connections.filter(
      (connection) =>
        !(connection.to.node === to.node && connection.to.port === to.port),
    );
    if (!exists) {
      voice.connections.push({ from, to });
    }
    if (movingWire) {
      const originalTarget = voice.nodes[movingWire.to.node];
      const originalInput = originalTarget
        ? allInputList(originalTarget).find(
            (candidate) => candidate.id === movingWire?.to.port,
          )
        : undefined;
      for (const connection of displaced) {
        const source = voice.nodes[connection.from.node];
        const sourcePort = source
          ? allOutputList(source).find(
              (candidate) => candidate.id === connection.from.port,
            )
          : undefined;
        if (!source || !sourcePort || !originalInput) continue;
        if (sourcePort.ty !== originalInput.ty) continue;
        voice.connections.push({
          from: { ...connection.from },
          to: { ...movingWire.to },
        });
        break;
      }
      movingWire = null;
    }
    touchVoice();
  }

  function openNodeMenu(event: MouseEvent, node: VoiceNode) {
    event.preventDefault();
    event.stopPropagation();
    if (!selected.has(node.id)) selectOnly(node.id);
    active = node.id;
    addMenu = null;
    nodeMenu = { x: event.clientX, y: event.clientY, nodeId: node.id };
  }

  function nodeResizeDown(event: MouseEvent, node: VoiceNode): void {
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    const ids = selected.has(node.id) ? [...selected] : [node.id];
    const widths: Record<string, number> = {};
    for (const id of ids) {
      const candidate = voice.nodes[id];
      if (candidate) widths[id] = nodeWidth(candidate);
    }
    nodeResize = { ids, startX: event.clientX, widths };
  }

  function zoneMouseDown(event: MouseEvent, header: VoiceNode): void {
    if (event.button !== 0) return;
    const target = event.target as Element | null;
    if (target?.closest("input, button, select, textarea")) return;
    event.preventDefault();
    event.stopPropagation();
    selectOnly(header.id);
    const ids = [header.id, ...zoneMemberIds(header.id)];
    const origins: Record<string, Point> = {};
    for (const id of ids) {
      const position = positions[id];
      if (position) origins[id] = { ...position };
    }
    drag = {
      ids,
      startX: event.clientX,
      startY: event.clientY,
      origins,
      moved: false,
    };
  }

  function nodeMouseDown(event: MouseEvent, node: VoiceNode) {
    if (circleMode) return;
    if (wire) {
      wire = null;
      wireMoved = false;
      pendingLink = null;
      restoreMovingWire();
    }
    nodeMenu = null;
    if (event.button !== 0) return;
    const target = event.target as Element | null;
    if (target?.closest("input, button, select, textarea")) return;
    event.preventDefault();
    if (event.shiftKey) {
      const next = new Set(selected);
      if (next.has(node.id)) next.delete(node.id);
      else next.add(node.id);
      selected = next;
      active = node.id;
    } else if (event.ctrlKey || event.metaKey) {
      selectOnly(node.id);
    } else if (!selected.has(node.id)) {
      selectOnly(node.id);
    } else {
      active = node.id;
    }
    if (event.altKey) {
      voice.connections = voice.connections.filter(
        (connection) =>
          !selected.has(connection.from.node) &&
          !selected.has(connection.to.node),
      );
    }
    const dragIds = new Set(selected);
    for (const id of [...dragIds]) {
      if (voice.nodes[id]?.type_id === "voice.zone.repeat") {
        for (const member of zoneMemberIds(id)) dragIds.add(member);
      }
    }
    const origins: Record<string, Point> = {};
    for (const id of dragIds) {
      const position = positions[id];
      if (position) origins[id] = { ...position };
    }
    drag = {
      ids: [...dragIds],
      startX: event.clientX,
      startY: event.clientY,
      origins,
      moved: false,
    };
  }

  function move(event: MouseEvent) {
    lastPointer = { x: event.clientX, y: event.clientY };
    if (circleMode) {
      const point = boardPoint(event);
      if (point) {
        circlePos = point;
        if (circleDragging) selectInCircle(point, event.shiftKey);
      }
      return;
    }
    if (nodeResize) {
      const dx = (event.clientX - nodeResize.startX) / view.scale;
      for (const id of nodeResize.ids) {
        const node = voice.nodes[id];
        if (!node) continue;
        const width = (nodeResize.widths[id] ?? NODE_WIDTH) + dx;
        node.width = Math.max(140, Math.min(480, Math.round(width)));
      }
      return;
    }
    if (cutLine) {
      const point = boardPoint(event);
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
    if (muteLine) {
      const point = boardPoint(event);
      if (point) {
        const previous = muteLine.at(-1);
        if (
          !previous ||
          Math.hypot(point.x - previous.x, point.y - previous.y) > 2
        ) {
          muteLine = [...muteLine, point];
        }
      }
      return;
    }
    if (wire) {
      wireMoved = true;
      const point = worldPoint(event);
      if (point) wire = { ...wire, x: point.x, y: point.y };
      return;
    }
    if (lasso) {
      const point = boardPoint(event);
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
    if (pan) {
      view = {
        ...view,
        x: pan.viewX + event.clientX - pan.startX,
        y: pan.viewY + event.clientY - pan.startY,
      };
      return;
    }
    if (marquee) {
      const point = boardPoint(event);
      if (point) marquee = { ...marquee, x2: point.x, y2: point.y };
      return;
    }
    if (frameDrag) {
      const dx = (event.clientX - frameDrag.startX) / view.scale;
      const dy = (event.clientY - frameDrag.startY) / view.scale;
      const frame = voice.frames?.[frameDrag.id];
      if (frame) {
        frame.position = [frameDrag.origin[0] + dx, frameDrag.origin[1] + dy];
      }
      for (const [id, origin] of Object.entries(frameDrag.children)) {
        const node = voice.nodes[id];
        if (node) node.position = [origin[0] + dx, origin[1] + dy];
      }
      return;
    }
    if (zoneResize) {
      const header = voice.nodes[zoneResize.id];
      if (header) {
        header.width = Math.max(
          320,
          zoneResize.width + (event.clientX - zoneResize.startX) / view.scale,
        );
        header.height = Math.max(
          160,
          zoneResize.height + (event.clientY - zoneResize.startY) / view.scale,
        );
      }
      return;
    }
    if (frameResize) {
      const frame = voice.frames?.[frameResize.id];
      if (frame) {
        frame.size = [
          Math.max(160, frameResize.size[0] + (event.clientX - frameResize.startX) / view.scale),
          Math.max(100, frameResize.size[1] + (event.clientY - frameResize.startY) / view.scale),
        ];
      }
      return;
    }
    if (!drag) return;
    const dx = (event.clientX - drag.startX) / view.scale;
    const dy = (event.clientY - drag.startY) / view.scale;
    if (!drag.moved && Math.hypot(dx, dy) < 3) return;
    drag.moved = true;
    for (const id of drag.ids) {
      const node = voice.nodes[id];
      const origin = drag.origins[id];
      if (!node || !origin) continue;
      node.position = [origin.x + dx, origin.y + dy];
    }
    insertTarget =
      drag.ids.length === 1 && voice.nodes[drag.ids[0] ?? ""]
        ? findInsertTarget(voice.nodes[drag.ids[0] ?? ""]!)
        : null;
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
      const point = boardPoint(event);
      if (!point) return;
      circlePos = point;
      circleDragging = true;
      selectInCircle(point, event.shiftKey);
      return;
    }
    if (event.button === 2 && event.ctrlKey && event.altKey) {
      event.preventDefault();
      const point = boardPoint(event);
      if (point) {
        muteLine = [point];
        addMenu = null;
        nodeMenu = null;
      }
      return;
    }
    if (event.button === 2 && event.ctrlKey) {
      event.preventDefault();
      const point = boardPoint(event);
      if (point) {
        cutLine = [point];
        addMenu = null;
        nodeMenu = null;
      }
      return;
    }
    if (event.button === 1) {
      event.preventDefault();
      pan = {
        startX: event.clientX,
        startY: event.clientY,
        viewX: view.x,
        viewY: view.y,
      };
      return;
    }
    const target = event.target as Element | null;
    if (wire) {
      wire = null;
      wireMoved = false;
      pendingLink = null;
      return;
    }
    const zoneHead = target?.closest<HTMLElement>(".voice-zone-head");
    if (zoneHead?.dataset.zoneId) {
      const header = voice.nodes[zoneHead.dataset.zoneId];
      if (header) zoneMouseDown(event, header);
      return;
    }
    if (
      target?.closest(
        ".voice-zone-head, .voice-node, .voice-add-menu, .voice-interface-panel, .voice-wire-hit",
      )
    ) {
      return;
    }
    addMenu = null;
    nodeMenu = null;
    if (event.button !== 0) return;
    if (event.button === 0 && event.ctrlKey && event.altKey) {
      event.preventDefault();
      const point = boardPoint(event);
      if (point) {
        lasso = {
          points: [point],
          subtract: event.shiftKey,
        };
      }
      return;
    }
    if (event.altKey) {
      event.preventDefault();
      pan = {
        startX: event.clientX,
        startY: event.clientY,
        viewX: view.x,
        viewY: view.y,
      };
      return;
    }
    const point = boardPoint(event);
    if (!point) return;
    if (!event.shiftKey && !event.ctrlKey && !event.metaKey) {
      selected = new Set();
      active = "";
    }
    marquee = { x1: point.x, y1: point.y, x2: point.x, y2: point.y };
  }

  function handleWheel(event: WheelEvent) {
    event.preventDefault();
    if (circleMode) {
      circleRadius = Math.max(
        12,
        Math.min(240, circleRadius - event.deltaY * 0.08),
      );
      return;
    }
    const point = boardPoint(event);
    if (!point) return;
    const factor = Math.exp(-event.deltaY * 0.0012);
    const scale = Math.min(2.5, Math.max(0.25, view.scale * factor));
    const ratio = scale / view.scale;
    view = {
      scale,
      x: point.x - (point.x - view.x) * ratio,
      y: point.y - (point.y - view.y) * ratio,
    };
  }

  function selectInCircle(center: Point, subtract: boolean) {
    const next = new Set(selected);
    for (const node of nodes) {
      const position = positions[node.id];
      if (!position) continue;
      const point = {
        x: view.x + (position.x + nodeWidth(node) / 2) * view.scale,
        y: view.y + (position.y + nodeHeight(node) / 2) * view.scale,
      };
      if (Math.hypot(point.x - center.x, point.y - center.y) <= circleRadius) {
        if (subtract) next.delete(node.id);
        else next.add(node.id);
      }
    }
    selected = next;
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

  function marqueeStyle(): string {
    if (!marquee) return "";
    const left = Math.min(marquee.x1, marquee.x2);
    const top = Math.min(marquee.y1, marquee.y2);
    const width = Math.abs(marquee.x2 - marquee.x1);
    const height = Math.abs(marquee.y2 - marquee.y1);
    return `left:${left}px;top:${top}px;width:${width}px;height:${height}px;`;
  }

  function end(event?: MouseEvent) {
    if (circleMode) {
      circleDragging = false;
      return;
    }
    if (cutLine) {
      const line = cutLine;
      cutLine = null;
      if (line.length > 1) {
        voice.connections = voice.connections.filter(
          (connection) => !cutWire(connection, line),
        );
      }
      return;
    }
    if (muteLine) {
      const line = muteLine;
      muteLine = null;
      if (line.length > 1) {
        for (const connection of voice.connections) {
          if (cutWire(connection, line)) connection.muted = !connection.muted;
        }
        touchVoice();
      }
      return;
    }
    if (wire && event) {
      const target =
        (event.target as Element | null)?.closest<HTMLElement>(".voice-port") ??
        document
          .elementFromPoint(event.clientX, event.clientY)
          ?.closest<HTMLElement>(".voice-port");
      const targetNode = target?.dataset.portNode;
      const targetPort = target?.dataset.portId;
      const targetSide = target?.dataset.portSide;
      if (
        targetNode &&
        targetPort &&
        targetSide &&
        targetSide !== wire.side
      ) {
        const node = voice.nodes[targetNode];
        const port = node
          ? (targetSide === "in" ? inputList(node) : outputList(node)).find(
              (candidate) => candidate.id === targetPort,
            )
          : undefined;
        if (node && port) finishWire(event, node, port);
      } else if (movingWire) {
        restoreMovingWire();
        wire = null;
      } else if (wireMoved && boardElement) {
        const point = boardPoint(event);
        const rect = boardElement.getBoundingClientRect();
        if (
          point &&
          point.x >= 0 &&
          point.y >= 0 &&
          point.x <= rect.width &&
          point.y <= rect.height
        ) {
          pendingLink = wire;
          addMenu = { x: event.clientX, y: event.clientY };
          wire = null;
        }
      }
    }
    if (lasso) {
      const area = lasso;
      lasso = null;
      if (area.points.length > 2) {
        const next = new Set(selected);
        for (const node of nodes) {
          const position = positions[node.id];
          if (!position) continue;
          const center = {
            x: view.x + (position.x + nodeWidth(node) / 2) * view.scale,
            y:
              view.y +
              (position.y + nodeHeight(node) / 2) * view.scale,
          };
          if (pointInPolygon(center, area.points)) {
            if (area.subtract) next.delete(node.id);
            else next.add(node.id);
          }
        }
        selected = next;
      }
    }
    if (marquee) {
      const left = Math.min(marquee.x1, marquee.x2);
      const right = Math.max(marquee.x1, marquee.x2);
      const top = Math.min(marquee.y1, marquee.y2);
      const bottom = Math.max(marquee.y1, marquee.y2);
      const next = new Set(selected);
      for (const node of nodes) {
        const position = positions[node.id];
        if (!position) continue;
        const x = view.x + position.x * view.scale;
        const y = view.y + position.y * view.scale;
        const width = nodeWidth(node) * view.scale;
        const height = nodeHeight(node) * view.scale;
        if (
          x < right &&
          x + width > left &&
          y < bottom &&
          y + height > top
        ) {
          next.add(node.id);
        }
      }
      selected = next;
      marquee = null;
    }
    if (
      drag &&
      drag.moved &&
      drag.ids.length === 1 &&
      insertTarget &&
      voice.nodes[drag.ids[0] ?? ""]
    ) {
      insertNodeOnLink(voice.nodes[drag.ids[0] ?? ""]!, insertTarget);
    }
    const layoutChanged = !!(
      drag ||
      frameDrag ||
      frameResize ||
      zoneResize ||
      nodeResize
    );
    if (drag?.moved) {
      for (const id of drag.ids) {
        const node = voice.nodes[id];
        if (
          !node ||
          node.type_id === "voice.zone.repeat" ||
          node.type_id === "voice.zone.input" ||
          node.type_id === "voice.zone.output"
        ) {
          continue;
        }
        const [x, y] = node.position ?? [0, 0];
        const zone = zoneAtPoint(
          [x + nodeWidth(node) / 2, y + nodeHeight(node) / 2],
          id,
        );
        if (zone && !node.zone) node.zone = zone;
      }
    }
    insertTarget = null;
    pan = null;
    drag = null;
    frameDrag = null;
    frameResize = null;
    zoneResize = null;
    nodeResize = null;
    wire = null;
    if (layoutChanged) touchVoice();
  }

  async function createAndInsertGroup() {
    const name = newGroupName.trim();
    if (!name) return;
    libraryError = "";
    if (!(await save(false))) return;
    try {
      const id = await session.createGroup(name);
      addGroupNode(id);
      newGroupName = "";
      libraryOpen = false;
    } catch (error) {
      libraryError = errorMessage(error);
    }
  }

  function insertGroup(groupId: string) {
    addGroupNode(groupId);
    libraryOpen = false;
  }

  $effect(() => {
    const snapshot = JSON.stringify(voice);
    if (!autoSaveReady) {
      autoSaveReady = true;
      return;
    }
    void snapshot;
    if (autoSaveTimer) clearTimeout(autoSaveTimer);
    autoSaveTimer = setTimeout(() => {
      void save(false);
    }, 500);
  });

  onDestroy(() => {
    if (autoSaveTimer) clearTimeout(autoSaveTimer);
    if (interfaceTimer) clearTimeout(interfaceTimer);
  });

  function openInterface() {
    interfaceDraft = JSON.parse(
      JSON.stringify(groupInterface),
    ) as NodeGroupInterface;
    interfaceError = "";
    sidebarOpen = true;
    interfaceOpen = true;
  }

  function toggleSidebar() {
    sidebarOpen = !sidebarOpen;
  }

  async function renameCurrentGroup() {
    const name = groupNameDraft.trim();
    if (!name || name === groups[groupId]?.name) return;
    try {
      await session.renameGroup(groupId, name);
      error = "";
    } catch (caught) {
      error = errorMessage(caught);
    }
  }

  async function deleteCurrentGroup() {
    if (
      !window.confirm(
        `${t("voice.confirmDeleteGroup")} ${groupNameDraft || groupId}`,
      )
    ) {
      return;
    }
    try {
      await session.deleteGroup(groupId);
      onClose();
    } catch (caught) {
      error = errorMessage(caught);
    }
  }

  function addSocket(side: "inputs" | "outputs") {
    const list = interfaceDraft[side];
    const prefix = side === "inputs" ? "in" : "out";
    let number = list.length + 1;
    while (list.some((socket) => socket.id === `${prefix}-${number}`)) {
      number += 1;
    }
    list.push({
      id: `${prefix}-${number}`,
      name: `${tLoose(`socket.${prefix}`, prefix.toUpperCase())} ${number}`,
      type: side === "outputs" ? "audio" : "float",
      ...(side === "outputs"
        ? {}
        : { default: 0, step: 0.01 }),
    });
    queueInterfaceSave();
  }

  function removeSocket(side: "inputs" | "outputs", index: number) {
    interfaceDraft[side].splice(index, 1);
    queueInterfaceSave();
  }

  let interfaceTimer: ReturnType<typeof setTimeout> | null = null;

  async function saveInterface(close = true) {
    interfaceError = "";
    try {
      await session.setGroupInterface(groupId, interfaceDraft);
      if (close) interfaceOpen = false;
    } catch (error) {
      interfaceError = errorMessage(error);
    }
  }

  function queueInterfaceSave() {
    if (interfaceTimer) clearTimeout(interfaceTimer);
    interfaceTimer = setTimeout(() => {
      interfaceTimer = null;
      void saveInterface(false);
    }, 500);
  }

  function setParam(node: VoiceNode, name: string, value: number) {
    node.params[name] = Number.isFinite(value) ? value : 0;
  }

  function touchVoice(): void {
    voice.nodes = { ...voice.nodes };
    voice.connections = [...voice.connections];
  }

  function addPosition(): [number, number] {
    if (!boardElement || !addMenu) return [80, 80];
    const point = worldPoint({
      clientX: addMenu.x,
      clientY: addMenu.y,
    });
    if (!point) return [80, 80];
    return [Math.max(10, point.x - 20), Math.max(10, point.y - 10)];
  }

  function connectPending(node: VoiceNode) {
    const link = pendingLink;
    pendingLink = null;
    if (!link) return;
    if (link.side === "out") {
      const target = inputList(node).find((port) => port.ty === link.ty);
      if (!target) return;
      if (!target.multi) {
        voice.connections = voice.connections.filter(
          (connection) =>
            !(
              connection.to.node === node.id &&
              connection.to.port === target.id
            ),
        );
      }
      voice.connections.push({
        from: { node: link.node, port: link.port },
        to: { node: node.id, port: target.id },
      });
    } else {
      const source = outputList(node).find((port) => port.ty === link.ty);
      if (!source) return;
      const targetNode = voice.nodes[link.node];
      const target = targetNode
        ? inputList(targetNode).find((port) => port.id === link.port)
        : undefined;
      if (!target?.multi) {
        voice.connections = voice.connections.filter(
          (connection) =>
            !(
              connection.to.node === link.node &&
              connection.to.port === link.port
            ),
        );
      }
      voice.connections.push({
        from: { node: node.id, port: source.id },
        to: { node: link.node, port: link.port },
      });
    }
    touchVoice();
  }

  function createZone() {
    const id = `zone-${Date.now().toString(36)}`;
    const position = addPosition();
    const header: VoiceNode = {
      id,
      type_id: "voice.zone.repeat",
      params: { count: 10 },
      position,
      width: 560,
      height: 320,
    };
    voice.nodes[id] = header;
    const input: VoiceNode = {
      id: `${id}-in`,
      type_id: "voice.zone.input",
      params: {},
      position: [position[0] + 24, position[1] + 78],
      zone: id,
    };
    const output: VoiceNode = {
      id: `${id}-out`,
      type_id: "voice.zone.output",
      params: {},
      position: [position[0] + 360, position[1] + 78],
      zone: id,
    };
    voice.nodes[input.id] = input;
    voice.nodes[output.id] = output;
    touchVoice();
    selectOnly(id);
    addMenu = null;
  }

  function addNode(kind: string) {
    if (kind === "voice.zone.repeat") {
      createZone();
      return;
    }
    const id = `voice-${Date.now().toString(36)}`;
    const params: Record<string, number> = {};
    for (const param of typeById.get(kind)?.params ?? []) {
      params[param.name] = param.default;
    }
    const position = addPosition();
    const zone = zoneAtPoint(position);
    const node: VoiceNode = {
      id,
      type_id: kind,
      params,
      position,
      zone,
    };
    voice.nodes[id] = node;
    connectPending(node);
    selectOnly(id);
    addMenu = null;
  }

  function addGroupNode(groupId: string) {
    const id = `voice-${Date.now().toString(36)}`;
    const node: VoiceNode = {
      id,
      type_id: "voice.group",
      params: {},
      position: addPosition(),
      group: groupId,
    };
    voice.nodes[id] = node;
    connectPending(node);
    selectOnly(id);
    addMenu = null;
  }

  function swapSelectedNode(node: VoiceNode, typeId: string): void {
    if (node.type_id === "group.input" || node.type_id === "group.output") {
      return;
    }
    const info = typeById.get(typeId);
    if (!info) return;
    const oldInputs = allInputList(node);
    const oldOutputs = allOutputList(node);
    const incoming = voice.connections.filter(
      (connection) => connection.to.node === node.id,
    );
    const outgoing = voice.connections.filter(
      (connection) => connection.from.node === node.id,
    );
    const params: Record<string, number> = {};
    for (const param of info.params) {
      params[param.name] = node.params[param.name] ?? param.default;
    }
    const replacement: VoiceNode = {
      ...node,
      type_id: typeId,
      params,
      group: undefined,
    };
    voice.nodes[node.id] = replacement;
    const newInputs = allInputList(replacement);
    const newOutputs = allOutputList(replacement);
    const matchInput = (old: PortInfo) =>
      newInputs.find((port) => port.id === old.id) ??
      newInputs.find((port) => port.name === old.name && port.ty === old.ty) ??
      newInputs.find((port) => port.ty === old.ty);
    const matchOutput = (old: PortInfo) =>
      newOutputs.find((port) => port.id === old.id) ??
      newOutputs.find((port) => port.name === old.name && port.ty === old.ty) ??
      newOutputs.find((port) => port.ty === old.ty);
    const next = voice.connections.filter(
      (connection) =>
        connection.to.node !== node.id && connection.from.node !== node.id,
    );
    for (const connection of incoming) {
      const old = oldInputs.find((port) => port.id === connection.to.port);
      const target = old ? matchInput(old) : undefined;
      if (target) {
        next.push({ ...connection, to: { node: node.id, port: target.id } });
      }
    }
    for (const connection of outgoing) {
      const old = oldOutputs.find((port) => port.id === connection.from.port);
      const source = old ? matchOutput(old) : undefined;
      if (source) {
        next.push({ ...connection, from: { node: node.id, port: source.id } });
      }
    }
    voice.connections = next;
  }

  function swapSelected(typeId: string): void {
    for (const id of selected) {
      const node = voice.nodes[id];
      if (node) swapSelectedNode(node, typeId);
    }
    swapMenu = null;
    touchVoice();
  }

  function makeLinks(replace: boolean): void {
    const ordered = nodes
      .filter((node) => selected.has(node.id))
      .sort((left, right) => {
        const a = positions[left.id]?.x ?? 0;
        const b = positions[right.id]?.x ?? 0;
        return a - b;
      });
    if (ordered.length < 2) return;
    const occupied = new Set(
      voice.connections
        .filter((connection) => {
          if (connection.muted) return false;
          const target = voice.nodes[connection.to.node];
          return target ? !inputIsMulti(target, connection.to.port) : false;
        })
        .map((connection) => `${connection.to.node}\u0000${connection.to.port}`),
    );
    for (let sourceIndex = 0; sourceIndex < ordered.length; sourceIndex += 1) {
      const source = ordered[sourceIndex]!;
      for (let targetIndex = sourceIndex + 1; targetIndex < ordered.length; targetIndex += 1) {
        const target = ordered[targetIndex]!;
        let best:
          | { port: PortInfo; input: PortInfo }
          | undefined;
        for (const output of allOutputList(source)) {
          const candidates = allInputList(target).filter(
            (input) => input.ty === output.ty,
          );
          const input =
            candidates.find(
              (candidate) =>
                candidate.id === output.id || candidate.name === output.name,
            ) ?? candidates[0];
          if (input) {
            best = { port: output, input };
            break;
          }
        }
        if (!best) continue;
        const chosen = best;
        const key = `${target.id}\u0000${chosen.input.id}`;
        const multi = chosen.input.multi === true;
        if (!multi && occupied.has(key) && !replace) continue;
        if (replace) {
          voice.connections = voice.connections.filter(
            (connection) =>
              !(
                connection.to.node === target.id &&
                connection.to.port === chosen.input.id
              ),
          );
        }
        voice.connections.push({
          from: { node: source.id, port: chosen.port.id },
          to: { node: target.id, port: chosen.input.id },
        });
        if (!multi) occupied.add(key);
      }
    }
    touchVoice();
  }

  function deleteSelectedWithReconnect(): void {
    const removed = new Set(
      [...expandZoneSelection(selected)].filter(
        (id) => id !== "voice-in" && id !== "out",
      ),
    );
    if (removed.size === 0) return;
    const targetBusy = new Set(
      voice.connections
        .filter((connection) => {
          if (
            connection.muted ||
            removed.has(connection.from.node) ||
            removed.has(connection.to.node)
          ) {
            return false;
          }
          const target = voice.nodes[connection.to.node];
          return target
            ? !inputIsMulti(target, connection.to.port)
            : false;
        })
        .map((connection) => `${connection.to.node}\u0000${connection.to.port}`),
    );
    const upstream = voice.connections.filter(
      (connection) =>
        !connection.muted &&
        removed.has(connection.to.node) &&
        !removed.has(connection.from.node),
    );
    const downstream = voice.connections.filter(
      (connection) =>
        !connection.muted &&
        removed.has(connection.from.node) &&
        !removed.has(connection.to.node),
    );
    const next = voice.connections.filter(
      (connection) =>
        !removed.has(connection.from.node) && !removed.has(connection.to.node),
    );
    for (const input of upstream) {
      const source = voice.nodes[input.from.node];
      if (!source) continue;
      const outputPort = allOutputList(source).find(
        (port) => port.id === input.from.port,
      );
      if (!outputPort) continue;
      for (const output of downstream) {
        const target = voice.nodes[output.to.node];
        if (!target) continue;
        const inputPort = allInputList(target).find(
          (port) => port.id === output.to.port,
        );
        if (!inputPort || inputPort.ty !== outputPort.ty) continue;
        const key = `${target.id}\u0000${inputPort.id}`;
        const multi = inputPort.multi === true;
        if (!multi && targetBusy.has(key)) continue;
        if (target.id === source.id) continue;
        next.push({
          from: { ...input.from },
          to: { node: target.id, port: inputPort.id },
        });
        if (!multi) targetBusy.add(key);
        break;
      }
    }
    voice.connections = next;
    for (const id of removed) delete voice.nodes[id];
    selected = new Set();
    active = "";
    touchVoice();
  }

  function removeSelected() {
    const removable = [...expandZoneSelection(selected)].filter(
      (id) => id !== "voice-in" && id !== "out",
    );
    if (removable.length === 0) return;
    for (const id of removable) {
      delete voice.nodes[id];
    }
    voice.connections = voice.connections.filter(
      (connection) =>
        !removable.includes(connection.from.node) &&
        !removable.includes(connection.to.node),
    );
    selected = new Set();
    active = "";
  }

  function deleteFromMenu() {
    removeSelected();
  }

  function dedupeConnections(connections: VoiceConnection[]): VoiceConnection[] {
    const result: VoiceConnection[] = [];
    const indexes = new Map<string, number>();
    for (const connection of connections) {
      const target = voice.nodes[connection.to.node];
      const multi =
        target !== undefined &&
        allInputList(target).find((port) => port.id === connection.to.port)
          ?.multi === true;
      const key = multi
        ? `${connection.to.node}\u0000${connection.to.port}\u0000${connection.from.node}\u0000${connection.from.port}\u0000${connection.muted ? "muted" : "active"}`
        : `${connection.to.node}\u0000${connection.to.port}\u0000${connection.muted ? "muted" : "active"}`;
      const existing = indexes.get(key);
      if (existing === undefined) {
        indexes.set(key, result.length);
        result.push(connection);
      } else {
        result[existing] = connection;
      }
    }
    return result;
  }

  async function save(close = true): Promise<boolean> {
    saving = true;
    error = "";
    try {
      const deduped = dedupeConnections(voice.connections);
      if (
        deduped.length !== voice.connections.length ||
        deduped.some((connection, index) => connection !== voice.connections[index])
      ) {
        voice.connections = deduped;
      }
      await session.setGroupVoice(groupId, {
        nodes: voice.nodes,
        connections: voice.connections,
        frames: voice.frames ?? {},
      });
      if (close) onClose();
      return true;
    } catch (cause) {
      error = errorMessage(cause);
      return false;
    } finally {
      saving = false;
    }
  }

  async function makeGroup() {
    const ids = [...selected].filter(
      (id) => id !== "voice-in" && id !== "out",
    );
    if (ids.length === 0) {
      error = "先选中要成组的节点";
      return;
    }
    if (!(await save(false))) return;
    try {
      const result = await session.groupFromNodes(
        groupId,
        ids,
        t("voice.groupNode"),
      );
      selected = new Set();
      active = "";
      onEnterGroup(result.group_id);
    } catch (caught) {
      error = errorMessage(caught);
    }
  }

  type VoiceClipboard = {
    nodes: VoiceNode[];
    connections: VoiceConnection[];
  };
  let clipboard = $state<VoiceClipboard | null>(null);

  function cloneSelection(offset: readonly [number, number]) {
    const ids = new Set(
      [...selected].filter((id) => id !== "voice-in" && id !== "out"),
    );
    if (ids.size === 0) return [];
    const mapping = new Map<string, string>();
    const stamp = Date.now().toString(36);
    let index = 0;
    for (const id of ids) {
      const source = voice.nodes[id];
      if (!source) continue;
      const nextId = `voice-${stamp}-${index}`;
      index += 1;
      mapping.set(id, nextId);
      const position = source.position ?? [80, 80];
      voice.nodes[nextId] = {
        ...JSON.parse(JSON.stringify(source)) as VoiceNode,
        id: nextId,
        position: [position[0] + offset[0], position[1] + offset[1]],
      };
    }
    for (const connection of [...voice.connections]) {
      const from = mapping.get(connection.from.node);
      const to = mapping.get(connection.to.node);
      if (!from || !to) continue;
      voice.connections.push({
        from: { node: from, port: connection.from.port },
        to: { node: to, port: connection.to.port },
      });
    }
    const created = [...mapping.values()];
    selected = new Set(created);
    active = created.at(-1) ?? "";
    return created;
  }

  function duplicateSelected() {
    cloneSelection([26, 26]);
  }

  function createFrame() {
    const members = nodes.filter(
      (node) =>
        selected.has(node.id) && node.id !== "voice-in" && node.id !== "out",
    );
    let left = Infinity;
    let top = Infinity;
    let right = -Infinity;
    let bottom = -Infinity;
    for (const node of members) {
      const position = positions[node.id];
      if (!position) continue;
      left = Math.min(left, position.x);
      top = Math.min(top, position.y);
      right = Math.max(right, position.x + nodeWidth(node));
      bottom = Math.max(bottom, position.y + nodeHeight(node));
    }
    if (!Number.isFinite(left)) {
      const rect = boardElement?.getBoundingClientRect();
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
    voice.frames = { ...(voice.frames ?? {}), [frame.id]: frame };
    for (const node of members) node.frame = frame.id;
    touchVoice();
  }

  function unframeSelection() {
    for (const id of selected) {
      const node = voice.nodes[id];
      if (node) node.frame = undefined;
    }
    touchVoice();
  }

  function startFrameRename(frame: VoiceFrame): void {
    editingFrameId = frame.id;
    void tick().then(() => {
      frameNameInput?.focus();
      frameNameInput?.select();
    });
  }

  function commitFrameName(frame: VoiceFrame, value: string): void {
    const name = value.trim();
    if (name) frame.name = name;
    editingFrameId = null;
    touchVoice();
  }

  function frameMouseDown(event: MouseEvent, frame: VoiceFrame) {
    if (circleMode) return;
    if (event.button !== 0) return;
    if ((event.target as Element | null)?.closest("input")) return;
    event.preventDefault();
    event.stopPropagation();
    const children: Record<string, [number, number]> = {};
    for (const node of nodes) {
      if (node.frame === frame.id && node.position) {
        children[node.id] = [...node.position];
      }
    }
    frameDrag = {
      id: frame.id,
      startX: event.clientX,
      startY: event.clientY,
      origin: [...frame.position],
      children,
    };
  }

  function deleteFrame(frame: VoiceFrame) {
    if (!window.confirm(t("voice.confirmDeleteFrame"))) return;
    for (const node of nodes) {
      if (node.frame === frame.id) node.frame = undefined;
    }
    if (voice.frames) delete voice.frames[frame.id];
    touchVoice();
  }

  function zoneResizeDown(event: MouseEvent, header: VoiceNode) {
    if (circleMode) return;
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    const bounds = zoneBounds(header);
    zoneResize = {
      id: header.id,
      startX: event.clientX,
      startY: event.clientY,
      width: bounds.width,
      height: bounds.height,
    };
  }

  function frameResizeDown(event: MouseEvent, frame: VoiceFrame) {
    if (circleMode) return;
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    frameResize = {
      id: frame.id,
      startX: event.clientX,
      startY: event.clientY,
      size: [...frame.size],
    };
  }

  function toggleCollapsed() {
    for (const id of selected) {
      const node = voice.nodes[id];
      if (node) node.collapsed = !node.collapsed;
    }
  }

  function toggleMuted() {
    for (const id of selected) {
      const node = voice.nodes[id];
      if (node) node.muted = !node.muted;
    }
  }

  function copySelected() {
    const ids = new Set(
      [...selected].filter((id) => id !== "voice-in" && id !== "out"),
    );
    if (ids.size === 0) return;
    clipboard = {
      nodes: [...ids]
        .map((id) => voice.nodes[id])
        .filter((node): node is VoiceNode => Boolean(node))
        .map((node) => JSON.parse(JSON.stringify(node)) as VoiceNode),
      connections: voice.connections
        .filter(
          (connection) =>
            ids.has(connection.from.node) && ids.has(connection.to.node),
        )
        .map((connection) => JSON.parse(JSON.stringify(connection)) as VoiceConnection),
    };
  }

  function pasteClipboard() {
    const content = clipboard;
    if (!content || content.nodes.length === 0) return;
    const stamp = Date.now().toString(36);
    const mapping = new Map<string, string>();
    content.nodes.forEach((source, index) => {
      const nextId = `voice-${stamp}-${index}`;
      mapping.set(source.id, nextId);
      const position = source.position ?? [80, 80];
      voice.nodes[nextId] = {
        ...JSON.parse(JSON.stringify(source)) as VoiceNode,
        id: nextId,
        position: [position[0] + 30, position[1] + 30],
      };
    });
    for (const connection of content.connections) {
      const from = mapping.get(connection.from.node);
      const to = mapping.get(connection.to.node);
      if (!from || !to) continue;
      voice.connections.push({
        from: { node: from, port: connection.from.port },
        to: { node: to, port: connection.to.port },
      });
    }
    const created = [...mapping.values()];
    selected = new Set(created);
    active = created.at(-1) ?? "";
  }

  function sidebarParams(node: VoiceNode) {
    const params = [...(typeById.get(node.type_id)?.params ?? [])];
    if (node.type_id === "voice.group") {
      for (const socket of nodeGroup(node)?.interface.inputs ?? []) {
        if (socket.type === "audio" || socket.type === "note") continue;
        const param = matchingParam(node, socket.id);
        if (param) params.push(param);
      }
    }
    return params.filter((param) => !isLinked(node, param.name));
  }

  function reloadFromSession() {
    const group = session.nodeGroups[groupId];
    if (group) {
      voice = JSON.parse(JSON.stringify(group.graph)) as VoiceGraph;
    }
  }

  async function ungroupActive() {
    const node = voice.nodes[active];
    if (!node || node.type_id !== "voice.group") {
      error = "先选中一个组节点";
      return;
    }
    if (!(await save(false))) return;
    try {
      const result = await session.ungroupNode(groupId, node.id);
      reloadFromSession();
      selected = new Set(result.node_ids);
      active = "";
    } catch (caught) {
      error = errorMessage(caught);
    }
  }

  async function enterGroup(node: VoiceNode) {
    const group = nodeGroup(node);
    if (!group) return;
    if (await save(false)) onEnterGroup(group.id);
  }

  async function navigateTo(index: number) {
    if (await save(false)) onNavigate(index);
  }

  async function leave() {
    if (!(await save(false))) return;
    if (path.length > 1) onNavigate(path.length - 2);
    else onClose();
  }

  function frameAll() {
    if (!boardElement || nodes.length === 0) {
      view = { x: 0, y: 0, scale: 1 };
      return;
    }
    let left = Infinity;
    let top = Infinity;
    let right = -Infinity;
    let bottom = -Infinity;
    for (const node of nodes) {
      const position = positions[node.id] ?? { x: 0, y: 0 };
      left = Math.min(left, position.x);
      top = Math.min(top, position.y);
      right = Math.max(right, position.x + nodeWidth(node));
      bottom = Math.max(bottom, position.y + nodeHeight(node));
    }
    const rect = boardElement.getBoundingClientRect();
    const margin = 70;
    const width = Math.max(1, right - left);
    const height = Math.max(1, bottom - top);
    const scale = Math.min(
      1.6,
      Math.max(
        0.25,
        Math.min(
          (rect.width - margin * 2) / width,
          (rect.height - margin * 2) / height,
        ),
      ),
    );
    view = {
      scale,
      x: rect.width / 2 - ((left + right) / 2) * scale,
      y: rect.height / 2 - ((top + bottom) / 2) * scale,
    };
  }

  function handleKey(event: KeyboardEvent) {
    const focused = document.activeElement;
    const typing =
      !!hostElement?.contains(focused) &&
      (focused instanceof HTMLInputElement ||
        focused instanceof HTMLTextAreaElement ||
        focused instanceof HTMLSelectElement);
    if (
      (event.ctrlKey || event.metaKey) &&
      !event.altKey &&
      event.key.toLowerCase() === "c" &&
      !typing
    ) {
      event.preventDefault();
      copySelected();
      return;
    }
    if (
      (event.ctrlKey || event.metaKey) &&
      !event.altKey &&
      event.key.toLowerCase() === "v" &&
      !typing
    ) {
      event.preventDefault();
      pasteClipboard();
      return;
    }
    if (
      ((event.shiftKey && !event.ctrlKey && !event.metaKey) ||
        (event.ctrlKey && !event.altKey)) &&
      event.key.toLowerCase() === "d" &&
      !typing
    ) {
      event.preventDefault();
      duplicateSelected();
      return;
    }
    if (
      event.ctrlKey &&
      event.altKey &&
      event.key.toLowerCase() === "g" &&
      !typing
    ) {
      event.preventDefault();
      void ungroupActive();
      return;
    }
    if (
      event.ctrlKey &&
      !event.altKey &&
      !event.shiftKey &&
      event.key.toLowerCase() === "g" &&
      !typing
    ) {
      event.preventDefault();
      void makeGroup();
      return;
    }
    if (event.key === "Tab" && event.ctrlKey && !typing) {
      event.preventDefault();
      void leave();
      return;
    }
    if (
      event.key === "Tab" &&
      !typing &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey
    ) {
      event.preventDefault();
      if (event.shiftKey) return;
      const node = voice.nodes[active];
      if (node && isGroupNode(node) && nodeGroup(node)) {
        void enterGroup(node);
      } else {
        void leave();
      }
      return;
    }
    if (event.key === "F2" && !typing) {
      if (!voice.nodes[active]) return;
      event.preventDefault();
      sidebarOpen = true;
      interfaceOpen = false;
      void tick().then(() => {
        labelInput?.focus();
        labelInput?.select();
      });
      return;
    }
    if (
      event.key.toLowerCase() === "n" &&
      !typing &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey
    ) {
      event.preventDefault();
      toggleSidebar();
      return;
    }
    if (event.key === "Escape") {
      circleMode = false;
      circleDragging = false;
      circlePos = null;
      addMenu = null;
      swapMenu = null;
      nodeMenu = null;
      pendingLink = null;
      restoreMovingWire();
      wire = null;
      libraryOpen = false;
      interfaceOpen = false;
      sidebarOpen = false;
      return;
    }
    if (
      (event.shiftKey && event.key.toLowerCase() === "a") ||
      event.key === "F3"
    ) {
      event.preventDefault();
      addMenu = { x: lastPointer.x, y: lastPointer.y };
      return;
    }
    if (
      event.key.toLowerCase() === "a" &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey &&
      !event.shiftKey
    ) {
      event.preventDefault();
      selected = new Set(nodes.map((node) => node.id));
      return;
    }
    if (event.altKey && event.key.toLowerCase() === "a") {
      event.preventDefault();
      selected = new Set();
      active = "";
      return;
    }
    if (event.ctrlKey && event.key.toLowerCase() === "i") {
      event.preventDefault();
      selected = new Set(
        nodes.filter((node) => !selected.has(node.id)).map((node) => node.id),
      );
      return;
    }
    if (
      event.ctrlKey &&
      !event.altKey &&
      (event.key.toLowerCase() === "p" || event.key.toLowerCase() === "j")
    ) {
      event.preventDefault();
      createFrame();
      return;
    }
    if (event.altKey && event.key.toLowerCase() === "p") {
      event.preventDefault();
      unframeSelection();
      return;
    }
    if (
      event.key.toLowerCase() === "c" &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey &&
      !typing
    ) {
      event.preventDefault();
      circleMode = !circleMode;
      circleDragging = false;
      if (circleMode) {
        const rect = boardElement?.getBoundingClientRect();
        circlePos =
          boardPoint({
            clientX: lastPointer.x,
            clientY: lastPointer.y,
          }) ??
          (rect
            ? { x: rect.width / 2, y: rect.height / 2 }
            : { x: 200, y: 200 });
      } else {
        circlePos = null;
      }
      return;
    }
    if (
      event.shiftKey &&
      event.key.toLowerCase() === "s" &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey &&
      !typing
    ) {
      event.preventDefault();
      addMenu = null;
      swapMenu = { x: lastPointer.x, y: lastPointer.y };
      return;
    }
    if (
      event.key.toLowerCase() === "j" &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey &&
      !typing
    ) {
      event.preventDefault();
      makeLinks(event.shiftKey);
      return;
    }
    if (event.ctrlKey && event.key.toLowerCase() === "h" && !typing) {
      event.preventDefault();
      hideUnconnected = !hideUnconnected;
      return;
    }
    if (
      event.key.toLowerCase() === "h" &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey &&
      !typing
    ) {
      event.preventDefault();
      toggleCollapsed();
      return;
    }
    if (
      event.key.toLowerCase() === "m" &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey &&
      !typing
    ) {
      event.preventDefault();
      toggleMuted();
      return;
    }
    if (event.key === "Home") {
      event.preventDefault();
      frameAll();
      return;
    }
    if (event.key.toLowerCase() === "f") {
      const node = voice.nodes[active];
      const position = node?.position;
      if (node && position && boardElement) {
        event.preventDefault();
        const rect = boardElement.getBoundingClientRect();
        view = {
          ...view,
          x: rect.width / 2 - (position[0] + nodeWidth(node) / 2) * view.scale,
          y: rect.height / 2 - (position[1] + nodeHeight(node) / 2) * view.scale,
        };
      }
      return;
    }
    if (
      event.ctrlKey &&
      (event.key === "Delete" || event.key === "Backspace") &&
      !typing
    ) {
      event.preventDefault();
      deleteSelectedWithReconnect();
      return;
    }
    if (
      (event.key === "Delete" ||
        event.key === "Backspace" ||
        event.key.toLowerCase() === "x") &&
      !typing
    ) {
      event.preventDefault();
      removeSelected();
    }
  }
</script>

<svelte:window onmousemove={move} onmouseup={end} onkeydown={handleKey} />

<div class="voice-editor-host" class:embedded bind:this={hostElement}>
  <section class="voice-editor" class:embedded role="dialog" aria-modal={!embedded}>
    <header class="voice-editor-head">
      <div>
        <div class="voice-editor-path">
          <button class="voice-editor-crumb" onclick={() => void leave()}>
            {t("voice.trackPath")}
          </button>
          {#each path as entry, index (entry.id)}
            <b>/</b>
            <button
              class="voice-editor-crumb"
              class:current={index === path.length - 1}
              onclick={() => void navigateTo(index)}
            >
              {entry.name}
            </button>
          {/each}
        </div>
      </div>
      <div class="voice-editor-actions">
        <button class="ghost small" onclick={openInterface}>
          {t("voice.interface")}
        </button>
        <button class="ghost small" onclick={onClose}>
          {t("voice.close")}
        </button>
      </div>
    </header>

    {#if libraryOpen}
      <div class="voice-interface-panel">
        <div class="voice-interface-head">
          <strong>{t("voice.library")}</strong>
          <button class="ghost small" onclick={() => (libraryOpen = false)}>
            {t("voice.close")}
          </button>
        </div>
        <div class="voice-interface-row">
          <input
            class="voice-interface-name"
            placeholder={t("voice.newGroupName")}
            bind:value={newGroupName}
            onkeydown={(event) => {
              if (event.key === "Enter") void createAndInsertGroup();
            }}
          />
          <button
            class="ghost small"
            disabled={!newGroupName.trim()}
            onclick={() => void createAndInsertGroup()}
          >
            {t("voice.createGroup")}
          </button>
        </div>
        {#if libraryError}
          <p class="error-text">{libraryError}</p>
        {/if}
        <div class="voice-interface-section">
          <div class="voice-interface-title">
            <span>{t("voice.library")}</span>
          </div>
          {#each Object.values(groups) as group (group.id)}
            {#if group.id !== groupId}
              <div class="voice-interface-row">
                <span>{group.name}</span>
                <code>{group.id}</code>
                <button
                  class="ghost small"
                  onclick={() => insertGroup(group.id)}
                >
                  {t("voice.insertGroup")}
                </button>
              </div>
            {/if}
          {/each}
        </div>
      </div>
    {/if}

    {#if error}
      <p class="error-text">{error}</p>
    {/if}

    {#if addMenu}
      <NodeAddMenu
        x={addMenu.x}
        y={addMenu.y}
        nodeTypes={menuTypes}
        groups={groupChoices}
        groupLabel={t("voice.groupNode")}
        onPickType={(typeId) => addNode(typeId)}
        onPickGroup={(groupId) => addGroupNode(groupId)}
        onClose={() => {
          addMenu = null;
          pendingLink = null;
        }}
      />
    {/if}
    {#if swapMenu}
      <NodeAddMenu
        x={swapMenu.x}
        y={swapMenu.y}
        nodeTypes={swapTypes}
        onPickType={(typeId) => swapSelected(typeId)}
        onClose={() => (swapMenu = null)}
      />
    {/if}

    <div class="voice-editor-main">
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div
      class="voice-editor-board"
      role="application"
      aria-label={t("voice.title")}
      bind:this={boardElement}
      oncontextmenu={(event) => {
        event.preventDefault();
        if (event.ctrlKey) return;
        addMenu = { x: event.clientX, y: event.clientY };
      }}
      onmousedown={boardMouseDown}
      ondblclick={(event) => {
        const target = event.target as Element | null;
        if (
          !target?.closest(
            ".voice-node, .voice-frame, .node-sidebar, .node-add-menu",
          )
        ) {
          void leave();
        }
      }}
      onwheel={handleWheel}
      style:background-position={`${view.x}px ${view.y}px`}
      style:background-size={`${18 * view.scale}px ${18 * view.scale}px`}
    >
      <div
        class="voice-editor-canvas"
        style:transform={`translate(${view.x}px, ${view.y}px) scale(${view.scale})`}
      >
      {#each frames as frame (frame.id)}
        <div
          class="voice-frame"
          style:left={`${frame.position[0]}px`}
          style:top={`${frame.position[1]}px`}
          style:width={`${frame.size[0]}px`}
          style:height={`${frame.size[1]}px`}
          style:border-color={frame.color ?? undefined}
        >
          <div
            class="voice-frame-head"
            role="button"
            tabindex="0"
            aria-label={frame.name}
            style:background={frame.color
              ? `color-mix(in srgb, ${frame.color} 22%, #e7ebe7)`
              : undefined}
            onmousedown={(event) => frameMouseDown(event, frame)}
            oncontextmenu={(event) => {
              event.preventDefault();
              event.stopPropagation();
              deleteFrame(frame);
            }}
          >
            {#if editingFrameId === frame.id}
              <input
                bind:this={frameNameInput}
                class="voice-frame-name"
                value={frame.name}
                onblur={(event) =>
                  commitFrameName(
                    frame,
                    (event.currentTarget as HTMLInputElement).value,
                  )}
                onkeydown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    commitFrameName(
                      frame,
                      (event.currentTarget as HTMLInputElement).value,
                    );
                  } else if (event.key === "Escape") {
                    editingFrameId = null;
                  }
                }}
              />
            {:else}
              <span
                class="voice-frame-title"
                role="presentation"
                ondblclick={(event) => {
                  event.stopPropagation();
                  startFrameRename(frame);
                }}
              >{frame.name}</span>
            {/if}
            <input
              class="voice-frame-color"
              type="color"
              value={frame.color ?? "#b8c0c7"}
              title={t("voice.frameColor")}
              oninput={(event) => {
                frame.color = (event.currentTarget as HTMLInputElement).value;
                touchVoice();
              }}
            />
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
      {#each zoneHeaders() as header (header.id)}
        {@const bounds = zoneBounds(header)}
        <div
          class="voice-zone"
          class:selected={selected.has(header.id)}
          style:left={`${bounds.left}px`}
          style:top={`${bounds.top}px`}
          style:width={`${bounds.width}px`}
          style:height={`${bounds.height}px`}
        >
          <div
            class="voice-zone-head"
            role="button"
            tabindex="0"
            aria-label={t("voice.repeatZone")}
            data-zone-id={header.id}
            onmousedown={(event) => zoneMouseDown(event, header)}
          >
            <button
              class="voice-zone-toggle"
              type="button"
              aria-label={t("graph.collapse")}
              onclick={() => toggleZoneCollapsed(header)}
            >{header.collapsed ? "+" : "−"}</button>
            <span class="voice-zone-label">{t("voice.repeatZone")}</span>
          </div>

          {#if !header.collapsed}
            <div
              class="voice-zone-resize"
              role="button"
              tabindex="0"
              aria-label={t("voice.resizeFrame")}
              onmousedown={(event) => zoneResizeDown(event, header)}
            ></div>
          {/if}
        </div>
      {/each}
      <svg
        class="voice-editor-wires"
        viewBox={`${wireBounds.x} ${wireBounds.y} ${wireBounds.width} ${wireBounds.height}`}
        style:left={`${wireBounds.x}px`}
        style:top={`${wireBounds.y}px`}
        style:width={`${wireBounds.width}px`}
        style:height={`${wireBounds.height}px`}
        style:overflow="visible"
      >
        {#each voice.connections.filter((connection) => !isCollapsedConnection(connection)) as connection (connection.from.node + connection.from.port + connection.to.node + connection.to.port)}
          <path
            class="voice-wire-hit"
            d={wirePath(connection)}
            data-from-node={connection.from.node}
            data-from-port={connection.from.port}
            data-to-node={connection.to.node}
            data-to-port={connection.to.port}
            role="button"
            tabindex="0"
            aria-label={t("graph.removeWire")}
            onclick={(event) => removeConnection(event, connection)}
            onkeydown={(event) => {
              if (event.key === "Enter" || event.key === " ") {
                removeConnection(event, connection);
              }
            }}
          />
          <path
            class="voice-wire"
            class:muted={connection.muted}
            d={wirePath(connection)}
            style:stroke={wireColor(connection)}
          />
        {/each}
        {#if insertTarget}
          <path class="voice-wire-insert" d={wirePath(insertTarget)} />
        {/if}
        {#if wire}
          <path
            class="voice-wire-temp"
            d={tempWirePath()}
            style:stroke={socketColors[wire.ty]}
          />
        {/if}
      </svg>

      {#each nodes as node (node.id)}
        {@const position = positions[node.id]}
        {#if !isZoneChrome(node) && !isInCollapsedZone(node)}
        <div
          class="voice-node"
          class:selected={selected.has(node.id)}
          class:collapsed={node.collapsed}
          class:muted={node.muted}
          style:left={`${position?.x ?? 0}px`}
          style:top={`${position?.y ?? 0}px`}
          style:width={`${nodeWidth(node)}px`}
          style:height={`${nodeHeight(node)}px`}
          onmousedown={(event) => nodeMouseDown(event, node)}
          oncontextmenu={(event) => openNodeMenu(event, node)}
          role="group"
          tabindex="-1"
          data-category={typeById.get(node.type_id)?.category ?? ""}
        >
          <div
            class="voice-node-head"
            role="presentation"
            style:background={node.color
              ? `color-mix(in srgb, ${node.color} 58%, #222228)`
              : undefined}
            ondblclick={() => {
              if (isGroupNode(node)) void enterGroup(node);
            }}
          >
            <span class="voice-node-title">{nodeLabel(node)}</span>
            {#if node.muted}
              <span class="voice-node-muted" title={muteActionLabel(node)}>M</span>
            {/if}
            <span class="voice-node-id">{node.id}</span>
          </div>

          <div
            class="voice-node-resize"
            role="presentation"
            title={t("voice.resizeNode")}
            onmousedown={(event) => nodeResizeDown(event, node)}
          ></div>
          {#if !node.collapsed}
          {#if isGroupNode(node)}
            <div class="voice-node-group-line">
              <code>{node.group ?? "—"}</code>
              <button
                class="voice-node-enter"
                title={t("graph.editGroup")}
                onclick={() => void enterGroup(node)}
              >
                ↗
              </button>
            </div>
          {/if}

          {#if node.type_id === "voice.zone.input"}
            {@const header = node.zone ? voice.nodes[node.zone] : undefined}
            {@const countPort = header
              ? inputList(header).find((port) => port.id === "count")
              : undefined}
            {@const sockets = header
              ? zoneSocketPorts(header.zone_inputs, { id: "out", name: "Audio" })
              : []}
            {#if header && countPort}
              <div
                class="voice-socket-row in"
                style:top={`${NODE_HEADER + NODE_BODY_TOP}px`}
              >
                <button
                  class="voice-port in"
                  class:port-int={countPort.ty === "int"}
                  class:port-float={countPort.ty === "float"}
                  style:top={`${NODE_ROW / 2 - 4}px`}
                  title={`${countPort.id} (${countPort.ty})`}
                  aria-label={countPort.id}
                  data-port-node={header.id}
                  data-port-id={countPort.id}
                  data-port-side="in"
                  data-port-type={countPort.ty}
                  onmousedown={(event) => startWire(event, header, countPort, "in")}
                ></button>
                <span class="voice-socket-name in">{socketLabel(countPort.name)}</span>
                {#if isLinked(header, "count")}
                  <span class="voice-zone-linked">↗</span>
                {:else}
                  <div class="voice-socket-value">
                    <NumberField
                      value={header.params.count ?? countPort.default ?? 10}
                      min={countPort.minimum ?? 1}
                      max={countPort.maximum ?? 64}
                      step={countPort.step ?? 1}
                      precision={countPort.ty === "int" ? 0 : undefined}
                      onchange={(value) => {
                        setParam(header, "count", value);
                        touchVoice();
                      }}
                    />
                  </div>
                {/if}
              </div>
            {/if}
            {#each sockets as port, index (port.id)}
              <div
                class="voice-socket-row in"
                style:top={`${NODE_HEADER + NODE_BODY_TOP + (index + 1) * NODE_ROW}px`}
              >
                <button
                  class="voice-port in"
                  class:port-audio={port.ty === "audio"}
                  class:port-int={port.ty === "int"}
                  class:port-float={port.ty === "float"}
                  class:port-gate={port.ty === "gate"}
                  style:top={`${NODE_ROW / 2 - 4}px`}
                  title={`${port.id} (${port.ty})`}
                  aria-label={port.id}
                  data-port-node={header?.id}
                  data-port-id={port.id}
                  data-port-side="in"
                  data-port-type={port.ty}
                  onmousedown={(event) => header && startWire(event, header, port, "in")}
                ></button>
                <span class="voice-socket-name in">{socketLabel(port.name)}</span>
                <span class="voice-socket-name out">{socketLabel(port.name)}</span>
                <button
                  class="voice-port out"
                  class:port-audio={port.ty === "audio"}
                  class:port-int={port.ty === "int"}
                  class:port-float={port.ty === "float"}
                  class:port-gate={port.ty === "gate"}
                  style:top={`${NODE_ROW / 2 - 4}px`}
                  title={`${port.id} (${port.ty})`}
                  aria-label={port.id}
                  data-port-node={node.id}
                  data-port-id={port.id}
                  data-port-side="out"
                  data-port-type={port.ty}
                  onmousedown={(event) => startWire(event, node, port, "out")}
                ></button>
              </div>
            {/each}
          {:else if node.type_id === "voice.zone.output"}
            {@const header = node.zone ? voice.nodes[node.zone] : undefined}
            {@const sockets = header
              ? zoneSocketPorts(header.zone_outputs, { id: "in", name: "Audio" })
              : []}
            {#each sockets as port, index (port.id)}
              <div
                class="voice-socket-row in"
                style:top={`${NODE_HEADER + NODE_BODY_TOP + index * NODE_ROW}px`}
              >
                <button
                  class="voice-port in"
                  class:port-audio={port.ty === "audio"}
                  class:port-int={port.ty === "int"}
                  class:port-float={port.ty === "float"}
                  class:port-gate={port.ty === "gate"}
                  style:top={`${NODE_ROW / 2 - 4}px`}
                  title={`${port.id} (${port.ty})`}
                  aria-label={port.id}
                  data-port-node={node.id}
                  data-port-id={port.id}
                  data-port-side="in"
                  data-port-type={port.ty}
                  onmousedown={(event) => startWire(event, node, port, "in")}
                ></button>
                <span class="voice-socket-name in">{socketLabel(port.name)}</span>
                <span class="voice-socket-name out">{socketLabel(port.name)}</span>
                <button
                  class="voice-port out"
                  class:port-audio={port.ty === "audio"}
                  class:port-int={port.ty === "int"}
                  class:port-float={port.ty === "float"}
                  class:port-gate={port.ty === "gate"}
                  style:top={`${NODE_ROW / 2 - 4}px`}
                  title={`${port.id} (${port.ty})`}
                  aria-label={port.id}
                  data-port-node={header?.id}
                  data-port-id={port.id}
                  data-port-side="out"
                  data-port-type={port.ty}
                  onmousedown={(event) => header && startWire(event, header, port, "out")}
                ></button>
              </div>
            {/each}
          {:else}
          {#each inputList(node) as port, index (port.id)}
            {@const param = matchingParam(node, port.id)}
            {@const linked = isLinked(node, port.id)}
            <div
              class="voice-socket-row in"
              style:top={`${portTop(node, index)}px`}
            >
              <button
                class="voice-port in"
                class:port-audio={port.ty === "audio"}
                class:port-int={port.ty === "int"}
                class:port-float={port.ty === "float"}
                class:port-gate={port.ty === "gate"}
                class:port-note={port.ty === "note"}
                class:port-multi={port.multi}
                style:top={`${NODE_ROW / 2 - 4}px`}
                title={`${port.id} (${port.ty})${port.multi ? " · multi" : ""}`}
                aria-label={port.id}
                data-port-node={node.id}
                data-port-id={port.id}
                data-port-side="in"
                data-port-type={port.ty}
                onmousedown={(event) => startWire(event, node, port, "in")}
              ></button>
              <span class="voice-socket-name in">{socketLabel(port.name)}</span>
              {#if port.panel}
                <span class="voice-socket-panel">{port.panel}</span>
              {/if}
              {#if param && !linked}
                <div class="voice-socket-value">
                  <NumberField
                    value={node.params[param.name] ?? param.default}
                    min={param.minimum}
                    max={param.maximum}
                    step={param.step ?? 0.01}
                    onchange={(value) => setParam(node, param.name, value)}
                  />
                </div>
              {/if}
            </div>
          {/each}

          {#each outputList(node) as port, index (port.id)}
            <div
              class="voice-socket-row out"
              style:top={`${portTop(node, index)}px`}
            >
              <span class="voice-socket-name out">{socketLabel(port.name)}</span>
              <button
                class="voice-port out"
                class:port-audio={port.ty === "audio"}
                class:port-int={port.ty === "int"}
                class:port-float={port.ty === "float"}
                class:port-gate={port.ty === "gate"}
                class:port-note={port.ty === "note"}
                class:port-multi={port.multi}
                style:top={`${NODE_ROW / 2 - 4}px`}
                title={`${port.id} (${port.ty})${port.multi ? " · multi" : ""}`}
                aria-label={port.id}
                data-port-node={node.id}
                data-port-id={port.id}
                data-port-side="out"
                data-port-type={port.ty}
                onmousedown={(event) => startWire(event, node, port, "out")}
              ></button>
            </div>
          {/each}

          {/if}
          {#each extraParams(node) as param, index (param.name)}
            {@const top =
              NODE_HEADER +
              groupHeaderHeight(node) +
              NODE_BODY_TOP +
              (Math.max(inputList(node).length, outputList(node).length) +
                index) *
                NODE_ROW}
            <div class="voice-socket-row param" style:top={`${top}px`}>
              <span class="voice-socket-name">{paramLabel(param.name)}</span>
              <div class="voice-socket-value">
                <NumberField
                  value={node.params[param.name] ?? param.default}
                  min={param.minimum}
                  max={param.maximum}
                  step={param.step ?? 0.01}
                    onchange={(value) => setParam(node, param.name, value)}
                />
              </div>
            </div>
          {/each}
          {:else}
            {#each inputList(node) as port, index (port.id)}
              <button
                class="voice-port in"
                class:port-audio={port.ty === "audio"}
                class:port-int={port.ty === "int"}
                class:port-float={port.ty === "float"}
                class:port-gate={port.ty === "gate"}
                class:port-note={port.ty === "note"}
                class:port-multi={port.multi}
                style:top={`${portTop(node, index)}px`}
                title={`${port.id} (${port.ty})${port.multi ? " · multi" : ""}`}
                aria-label={port.id}
                data-port-node={node.id}
                data-port-id={port.id}
                data-port-side="in"
                data-port-type={port.ty}
                onmousedown={(event) => startWire(event, node, port, "in")}
              ></button>
            {/each}
            {#each outputList(node) as port, index (port.id)}
              <button
                class="voice-port out"
                class:port-audio={port.ty === "audio"}
                class:port-int={port.ty === "int"}
                class:port-float={port.ty === "float"}
                class:port-gate={port.ty === "gate"}
                class:port-note={port.ty === "note"}
                class:port-multi={port.multi}
                style:top={`${portTop(node, index)}px`}
                title={`${port.id} (${port.ty})${port.multi ? " · multi" : ""}`}
                aria-label={port.id}
                data-port-node={node.id}
                data-port-id={port.id}
                data-port-side="out"
                data-port-type={port.ty}
                onmousedown={(event) => startWire(event, node, port, "out")}
              ></button>
            {/each}
          {/if}
        </div>
        {/if}
      {/each}
      </div>
      {#if marquee}
        <div class="voice-marquee" style={marqueeStyle()}></div>
      {/if}
      {#if lasso}
        <svg class="voice-lasso" aria-hidden="true">
          <polyline
            points={lasso.points.map((point) => `${point.x},${point.y}`).join(" ")}
          />
        </svg>
      {/if}
      {#if cutLine}
        <svg class="voice-cut" aria-hidden="true">
          <polyline
            points={cutLine.map((point) => `${point.x},${point.y}`).join(" ")}
          />
        </svg>
      {/if}
      {#if muteLine}
        <svg class="voice-cut voice-mute-line" aria-hidden="true">
          <polyline
            points={muteLine.map((point) => `${point.x},${point.y}`).join(" ")}
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
    </div>

    {#if sidebarOpen}
      <aside class="node-sidebar" aria-label={t("graph.sidebar")}>
        <div class="node-sidebar-tabs">
          <button
            class:active={!interfaceOpen}
            onclick={() => (interfaceOpen = false)}
          >{t("graph.nodeTab")}</button>
          <button class:active={interfaceOpen} onclick={openInterface}>
            {t("graph.groupTab")}
          </button>
          <button
            class="node-sidebar-close"
            title={t("voice.close")}
            onclick={() => (sidebarOpen = false)}
          >×</button>
        </div>
        {#if interfaceOpen}

      <div class="voice-interface-panel">
        <div class="node-group-meta">
          <label>
            <span>{t("voice.groupName")}</span>
            <input
              class="voice-interface-name"
              bind:value={groupNameDraft}
              onchange={() => void renameCurrentGroup()}
              onkeydown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  void renameCurrentGroup();
                }
              }}
            />
          </label>
          <button
            class="ghost small danger"
            onclick={() => void deleteCurrentGroup()}
          >
            {t("voice.deleteGroup")}
          </button>
        </div>
        <div class="voice-interface-head">
          <strong>{t("voice.interface")}</strong>
          <button class="ghost small" onclick={() => (libraryOpen = true)}>
            {t("voice.library")}
          </button>
          <button class="ghost small" onclick={() => (interfaceOpen = false)}>
            {t("voice.close")}
          </button>
        </div>
        {#each interfaceSides as section (section.key)}
          {@const sockets = interfaceDraft[section.key]}
          <div class="voice-interface-section">
            <div class="voice-interface-title">
              <span>{section.label}</span>
              <button
                class="ghost small"
                onclick={() => addSocket(section.key)}
              >
                +
              </button>
            </div>
            {#each sockets as socket, index (socket.id)}
              <div class="voice-interface-row">
                <code>{socket.id}</code>
                <input
                  class="voice-interface-name"
                  value={socket.name}
                  oninput={(event) => {
                    socket.name = event.currentTarget.value;
                    queueInterfaceSave();
                  }}
                />
                <select
                  class="voice-interface-type"
                  value={socket.type}
                  onchange={(event) => {
                    socket.type = event.currentTarget
                      .value as NodeGroupInterface["inputs"][number]["type"];
                    if (socket.type === "audio" || socket.type === "note") {
                      socket.default = undefined;
                      socket.minimum = undefined;
                      socket.maximum = undefined;
                      socket.step = undefined;
                    } else {
                      socket.default ??= 0;
                      socket.step ??= socket.type === "gate" ? 1 : 0.01;
                    }
                    queueInterfaceSave();
                  }}
                >
                  <option value="float">{t("graph.typeFloat")}</option>
                  <option value="gate">{t("graph.typeGate")}</option>
                  <option value="audio">{t("graph.typeAudio")}</option>
                  <option value="note">{t("graph.typeNote")}</option>
                </select>
                <button
                  class="ghost small"
                  onclick={() => removeSocket(section.key, index)}
                >
                  ×
                </button>
              </div>
              <div class="voice-interface-extra">
                {#if socket.type === "float" || socket.type === "gate"}
                  <label title={t("voice.socketDefault")}>
                    <span>{t("voice.socketDefault")}</span>
                    <NumberField
                      value={socket.default ?? 0}
                      min={socket.minimum}
                      max={socket.maximum}
                      step={socket.step ?? (socket.type === "gate" ? 1 : 0.01)}
                        onchange={(value) => {
                        socket.default = value;
                        queueInterfaceSave();
                      }}
                    />
                  </label>
                  <label title={t("voice.socketMinimum")}>
                    <span>{t("voice.socketMinimum")}</span>
                    <NumberField
                      value={socket.minimum ?? 0}
                      step={0.01}
                        onchange={(value) => {
                        socket.minimum = value;
                        queueInterfaceSave();
                      }}
                    />
                  </label>
                  <label title={t("voice.socketMaximum")}>
                    <span>{t("voice.socketMaximum")}</span>
                    <NumberField
                      value={socket.maximum ?? 1}
                      step={0.01}
                        onchange={(value) => {
                        socket.maximum = value;
                        queueInterfaceSave();
                      }}
                    />
                  </label>
                  <label title={t("voice.socketStep")}>
                    <span>{t("voice.socketStep")}</span>
                    <NumberField
                      value={socket.step ?? 0.01}
                      step={0.001}
                      precision={3}
                      onchange={(value) => {
                        socket.step = value;
                        queueInterfaceSave();
                      }}
                    />
                  </label>
                {/if}
                <label class="wide" title={t("voice.socketPanel")}>
                  <span>{t("voice.socketPanel")}</span>
                  <input
                    class="voice-interface-name"
                    placeholder={t("voice.socketPanelHint")}
                    value={socket.panel ?? ""}
                    oninput={(event) => {
                      const value = event.currentTarget.value.trim();
                      socket.panel = value || undefined;
                      queueInterfaceSave();
                    }}
                  />
                </label>
                <label class="wide" title={t("voice.socketDescription")}>
                  <span>{t("voice.socketDescription")}</span>
                  <input
                    class="voice-interface-name"
                    value={socket.description ?? ""}
                    oninput={(event) => {
                      const value = event.currentTarget.value.trim();
                      socket.description = value || undefined;
                      queueInterfaceSave();
                    }}
                  />
                </label>
              </div>
            {/each}
          </div>
        {/each}
        {#if interfaceError}
          <p class="error-text">{interfaceError}</p>
        {/if}
        <div class="voice-interface-actions">
          <span class="node-sidebar-label">{t("voice.autoSaved")}</span>
        </div>
      </div>
        {:else if voice.nodes[active]}
          {@const node = voice.nodes[active]!}
          <div class="node-sidebar-section">
            <div class="node-sidebar-label">{t("graph.activeNode")}</div>
            <strong>{nodeLabel(node)}</strong>
            <code>{node.id}</code>
            <div class="node-sidebar-type">{node.type_id}</div>
            {#if node.type_id === "voice.zone.repeat"}
              {#each ZONE_SIDES as side (side)}
                {@const sockets = editableZoneSockets(node, side)}
                <div class="zone-interface-editor">
                  <div class="node-sidebar-label">
                    {side === "inputs"
                      ? t("voice.zoneInputs")
                      : t("voice.zoneOutputs")}
                  </div>
                  {#each sockets as socket, index (socket.id)}
                    <div
                      class="zone-interface-row"
                      role="group"
                      aria-label={socket.name}
                      ondragover={(event) => event.preventDefault()}
                      ondrop={(event) => dropZoneSocket(event, node, side, index)}
                    >
                      <span
                        class="zone-socket-handle"
                        draggable="true"
                        role="button"
                        tabindex="-1"
                        aria-label={socket.name}
                        ondragstart={(event) =>
                          startZoneSocketDrag(event, node, side, index)}
                        ondragend={() => (zoneSocketDrag = null)}
                      >⠿</span>
                      <input
                        value={socket.name}
                        aria-label={socket.name}
                        oninput={(event) =>
                          updateZoneSocket(node, side, index, {
                            name: event.currentTarget.value,
                          })}
                      />
                      <select
                        value={socket.type}
                        aria-label={socket.type}
                        onchange={(event) =>
                          updateZoneSocket(node, side, index, {
                            type: event.currentTarget.value as NodeTypeSocket["type"],
                          })}
                      >
                        <option value="audio">audio</option>
                        <option value="float">float</option>
                        <option value="gate">gate</option>
                      </select>
                      <button
                        class="ghost small"
                        type="button"
                        disabled={sockets.length <= 1}
                        aria-label={t("voice.removeSocket")}
                        onclick={() => removeZoneSocket(node, side, index)}
                      >×</button>
                    </div>
                  {/each}
                  <button
                    class="ghost small"
                    type="button"
                    onclick={() => addZoneSocket(node, side)}
                  >{t("voice.addSocket")}</button>
                </div>
              {/each}
            {/if}
            <div class="node-preview">
              <button
                class="ghost small"
                type="button"
                disabled={previewLoading}
                onclick={() => void loadPreview()}
              >{t("graph.preview")}</button>
              {#if previewError}
                <span class="node-preview-error">{previewError}</span>
              {:else if preview}
                <svg class="node-preview-plot" viewBox="0 0 132 36" aria-hidden="true">
                  <polyline points={previewPoints()} />
                </svg>
                <span class="node-preview-range">
                  {preview.minimum.toFixed(3)} … {preview.maximum.toFixed(3)}
                </span>
              {/if}
            </div>
            <label class="node-sidebar-param">
              <span>{t("graph.nodeLabel")}</span>
              <input
                bind:this={labelInput}
                class="node-sidebar-text"
                value={node.label ?? ""}
                placeholder={nodeLabel(node)}
                oninput={(event) => {
                  const value = (event.currentTarget as HTMLInputElement).value.trim();
                  node.label = value || undefined;
                  touchVoice();
                }}
              />
            </label>
            <label class="node-sidebar-param">
              <span>{t("graph.nodeColor")}</span>
              <span class="node-color-row">
                <input
                  class="node-color-input"
                  type="color"
                  value={node.color ?? "#4a4a52"}
                  oninput={(event) => {
                    node.color = (event.currentTarget as HTMLInputElement).value;
                    touchVoice();
                  }}
                />
                {#if node.color}
                  <button
                    class="ghost small"
                    type="button"
                    onclick={() => {
                      node.color = undefined;
                      touchVoice();
                    }}
                  >{t("graph.clearColor")}</button>
                {/if}
              </span>
            </label>
            {#if isGroupNode(node)}
              {#if node.type_id === "voice.repeat"}
                <select
                  class="node-sidebar-select"
                  value={node.group ?? ""}
                  onchange={(event) => {
                    const value = (event.currentTarget as HTMLSelectElement).value;
                    node.group = value || undefined;
                  }}
                >
                  <option value="">—</option>
                  {#each Object.values(groups) as candidate (candidate.id)}
                    <option value={candidate.id}>{candidate.name}</option>
                  {/each}
                </select>
              {:else}
                <div class="node-sidebar-group">
                  {nodeGroup(node)?.name ?? node.group ?? "—"}
                </div>
              {/if}
              <button class="ghost small" onclick={() => void enterGroup(node)}>
                {t("graph.editGroup")}
              </button>
            {/if}
            {#each sidebarParams(node) as param (param.name)}
              <label class="node-sidebar-param">
                <span>{paramLabel(param.name)}</span>
                <NumberField
                  value={node.params[param.name] ?? param.default}
                  min={param.minimum}
                  max={param.maximum}
                  step={param.step ?? 0.01}
                    onchange={(value) => setParam(node, param.name, value)}
                />
              </label>
            {/each}
          </div>
        {:else}
          <p class="node-sidebar-empty">{t("graph.noActiveNode")}</p>
        {/if}
      </aside>
    {/if}
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
        {#if isGroupNode(voice.nodes[nodeMenu.nodeId])}

          <button
            role="menuitem"
            onclick={() => {
              const node = voice.nodes[nodeMenu?.nodeId ?? ""];
              nodeMenu = null;
              if (node) void enterGroup(node);
            }}
          >
            {t("graph.editGroup")}
          </button>
          <button
            role="menuitem"
            onclick={() => {
              nodeMenu = null;
              void ungroupActive();
            }}
          >
            {t("voice.ungroup")}
          </button>
        {/if}
        <button
          role="menuitem"
          onclick={() => {
            nodeMenu = null;
            duplicateSelected();
          }}
        >
          {t("graph.duplicate")}
        </button>
        <button
          role="menuitem"
          onclick={() => {
            nodeMenu = null;
            toggleCollapsed();
          }}
        >
          {t("graph.collapse")}
        </button>
        <button
          role="menuitem"
          onclick={() => {
            nodeMenu = null;
            toggleMuted();
          }}
        >
          {muteActionLabel(voice.nodes[nodeMenu.nodeId])}
        </button>
        <button
          role="menuitem"
          class="danger"
          onclick={() => {
            nodeMenu = null;
            removeSelected();
          }}
        >
          {t("graph.delete")}
        </button>
      </div>
    {/if}

    {#if wire}
      <div class="voice-editor-tip">{t("voice.connecting")}</div>
    {/if}
  </section>
</div>
