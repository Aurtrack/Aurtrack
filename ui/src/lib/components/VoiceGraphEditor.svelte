<script lang="ts">
  import { onDestroy } from "svelte";
  import type {
    NodeGroup,
    NodeGroupInterface,
    NodeTypeInfo,
    VoiceConnection,
    VoiceFrame,
    VoiceGraph,
    VoiceNode,
  } from "../api";
  import { errorMessage } from "../api";
  import { t } from "../i18n.svelte";
  import { session } from "../session.svelte";
  import NodeAddMenu from "./NodeAddMenu.svelte";
  import NumberField from "./NumberField.svelte";

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

  const NODE_WIDTH = 204;
  const NODE_HEADER = 26;
  const NODE_ROW = 22;
  const NODE_BODY_TOP = 6;
  const NODE_BOTTOM = 7;
  const CANVAS_WIDTH = 1400;
  const CANVAS_HEIGHT = 620;

  type Point = { x: number; y: number };
  type Drag = {
    ids: string[];
    startX: number;
    startY: number;
    origins: Record<string, Point>;
  };
  type Wire = {
    node: string;
    port: string;
    side: "in" | "out";
    ty: PortType;
    x: number;
    y: number;
  };

  type PortType = "float" | "gate" | "audio";
  const socketColors: Record<PortType, string> = {
    float: "#a1a1a1",
    gate: "#cca6d6",
    audio: "#63c763",
  };
  type PortInfo = { id: string; name: string; ty: PortType };

  let typeById = $derived(
    new Map(nodeTypes.map((nodeType) => [nodeType.id, nodeType])),
  );
  let menuTypes = $derived(
    nodeTypes.filter(
      (nodeType) =>
        nodeType.id !== "group.input" &&
        nodeType.id !== "group.output" &&
        nodeType.id !== "voice.group",
    ),
  );

  function nodeGroup(node: VoiceNode): NodeGroup | undefined {
    return node.group ? groups[node.group] : undefined;
  }

  function nodeLabel(node: VoiceNode): string {
    if (node.type_id === "group.input") return "GROUP INPUT";
    if (node.type_id === "group.output") return "GROUP OUTPUT";
    if (node.type_id === "voice.group") return "VOICE GROUP";
    return (typeById.get(node.type_id)?.name ?? node.type_id).toUpperCase();
  }

  function interfacePorts(sockets: NodeGroupInterface["inputs"]): PortInfo[] {
    return sockets.map((socket) => ({
      id: socket.id,
      name: socket.name,
      ty: socket.type,
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
    }));
  }

  function inputList(node: VoiceNode): PortInfo[] {
    if (node.type_id === "group.input") return [];
    if (node.type_id === "group.output") {
      return interfacePorts(groupInterface.outputs);
    }
    if (node.type_id === "voice.group") {
      return interfacePorts(nodeGroup(node)?.interface.inputs ?? []);
    }
    return typePorts(node, "inputs");
  }

  function outputList(node: VoiceNode): PortInfo[] {
    if (node.type_id === "group.output") return [];
    if (node.type_id === "group.input") {
      return interfacePorts(groupInterface.inputs);
    }
    if (node.type_id === "voice.group") {
      return interfacePorts(nodeGroup(node)?.interface.outputs ?? []);
    }
    return typePorts(node, "outputs");
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
  let libraryOpen = $state(false);
  let libraryError = $state("");
  let newGroupName = $state("");
  let autoSaveReady = false;
  let autoSaveTimer: ReturnType<typeof setTimeout> | null = null;
  const interfaceSides = [
    { key: "inputs" as const, label: "INPUTS" },
    { key: "outputs" as const, label: "OUTPUTS" },
  ];
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
  let drag = $state<Drag | null>(null);
  let nodeMenu = $state<{ x: number; y: number; nodeId: string } | null>(null);
  let pendingLink = $state<Wire | null>(null);
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
  let wire = $state<Wire | null>(null);
  let error = $state("");
  let saving = $state(false);
  let addMenu = $state<{ x: number; y: number } | null>(null);
  let lastPointer = $state({ x: 0, y: 0 });
  let boardElement = $state<HTMLElement | null>(null);
  let hostElement = $state<HTMLElement | null>(null);
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

  function groupHeaderHeight(node: VoiceNode): number {
    return node.type_id === "voice.group" ? 18 : 0;
  }

  function matchingParam(node: VoiceNode, portId: string) {
    return typeById
      .get(node.type_id)
      ?.params.find((param) => param.name === portId);
  }

  function isLinked(node: VoiceNode, portId: string): boolean {
    return voice.connections.some(
      (connection) =>
        connection.to.node === node.id && connection.to.port === portId,
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
    if (node.collapsed) {
      return NODE_HEADER + 3 + index * 6;
    }
    return (
      NODE_HEADER +
      groupHeaderHeight(node) +
      NODE_BODY_TOP +
      index * NODE_ROW +
      NODE_ROW / 2
    );
  }

  function portTop(node: VoiceNode, index: number): number {
    return node.collapsed
      ? portY(node, index)
      : portY(node, index) - NODE_ROW / 2;
  }

  function nodeHeight(node: VoiceNode): number {
    if (node.collapsed) {
      return NODE_HEADER + 4 + collapsedRows(node) * 6;
    }
    return (
      NODE_HEADER +
      groupHeaderHeight(node) +
      NODE_BODY_TOP +
      rowCount(node) * NODE_ROW +
      NODE_BOTTOM
    );
  }

  function wireColor(connection: VoiceConnection): string {
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
    const x1 = from.x + NODE_WIDTH;
    const y1 = from.y + portY(source, outIndex);
    const x2 = to.x;
    const y2 = to.y + portY(target, inIndex);
    const bend = Math.max(40, Math.abs(x2 - x1) * 0.45);
    return `M ${x1} ${y1} C ${x1 + bend} ${y1}, ${x2 - bend} ${y2}, ${x2} ${y2}`;
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
      activeWire.side === "out" ? position.x + NODE_WIDTH : position.x;
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

  function startWire(
    event: MouseEvent,
    node: VoiceNode,
    port: PortInfo,
    side: "in" | "out",
  ) {
    event.preventDefault();
    event.stopPropagation();
    if (wire) {
      if (wire.side !== side) finishWire(event, node, port);
      else wire = null;
      return;
    }
    pendingLink = null;
    selectOnly(node.id);
    wireMoved = false;
    const point = worldPoint(event);
    if (!point) return;
    wire = {
      node: node.id,
      port: port.id,
      side,
      ty: port.ty,
      x: point.x,
      y: point.y,
    };
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
    if (from.node === to.node) return;
    const exists = voice.connections.some(
      (connection) =>
        connection.from.node === from.node &&
        connection.from.port === from.port &&
        connection.to.node === to.node &&
        connection.to.port === to.port,
    );
    voice.connections = voice.connections.filter(
      (connection) =>
        !(connection.to.node === to.node && connection.to.port === to.port),
    );
    if (!exists) {
      voice.connections.push({ from, to });
    }
  }

  function openNodeMenu(event: MouseEvent, node: VoiceNode) {
    event.preventDefault();
    event.stopPropagation();
    if (!selected.has(node.id)) selectOnly(node.id);
    active = node.id;
    addMenu = null;
    nodeMenu = { x: event.clientX, y: event.clientY, nodeId: node.id };
  }

  function nodeMouseDown(event: MouseEvent, node: VoiceNode) {
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
    const origins: Record<string, Point> = {};
    for (const id of selected) {
      const position = positions[id];
      if (position) origins[id] = { ...position };
    }
    drag = {
      ids: [...selected],
      startX: event.clientX,
      startY: event.clientY,
      origins,
    };
  }

  function move(event: MouseEvent) {
    lastPointer = { x: event.clientX, y: event.clientY };
    if (wire) {
      wireMoved = true;
      const point = worldPoint(event);
      if (point) wire = { ...wire, x: point.x, y: point.y };
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
    for (const id of drag.ids) {
      const node = voice.nodes[id];
      const origin = drag.origins[id];
      if (!node || !origin) continue;
      node.position = [origin.x + dx, origin.y + dy];
    }
  }

  function boardMouseDown(event: MouseEvent) {
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
    if (target?.closest(".voice-node, .voice-add-menu, .voice-interface-panel")) {
      return;
    }
    addMenu = null;
    nodeMenu = null;
    if (event.button !== 0) return;
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

  function marqueeStyle(): string {
    if (!marquee) return "";
    const left = Math.min(marquee.x1, marquee.x2);
    const top = Math.min(marquee.y1, marquee.y2);
    const width = Math.abs(marquee.x2 - marquee.x1);
    const height = Math.abs(marquee.y2 - marquee.y1);
    return `left:${left}px;top:${top}px;width:${width}px;height:${height}px;`;
  }

  function end(event?: MouseEvent) {
    if (wire && event) {
      const element = document.elementFromPoint(event.clientX, event.clientY);
      const target = element?.closest<HTMLElement>(".voice-port");
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
        const width = NODE_WIDTH * view.scale;
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
    pan = null;
    drag = null;
    frameDrag = null;
    frameResize = null;
    wire = null;
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
      name: `${prefix.toUpperCase()} ${number}`,
      type: side === "outputs" ? "audio" : "float",
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
      voice.connections = voice.connections.filter(
        (connection) =>
          !(connection.to.node === node.id && connection.to.port === target.id),
      );
      voice.connections.push({
        from: { node: link.node, port: link.port },
        to: { node: node.id, port: target.id },
      });
    } else {
      const source = outputList(node).find((port) => port.ty === link.ty);
      if (!source) return;
      voice.connections = voice.connections.filter(
        (connection) =>
          !(connection.to.node === link.node && connection.to.port === link.port),
      );
      voice.connections.push({
        from: { node: node.id, port: source.id },
        to: { node: link.node, port: link.port },
      });
    }
  }

  function addNode(kind: string) {
    const id = `voice-${Date.now().toString(36)}`;
    const params: Record<string, number> = {};
    for (const param of typeById.get(kind)?.params ?? []) {
      params[param.name] = param.default;
    }
    const node: VoiceNode = {
      id,
      type_id: kind,
      params,
      position: addPosition(),
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

  function removeSelected() {
    const removable = [...selected].filter(
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

  async function save(close = true): Promise<boolean> {
    saving = true;
    error = "";
    try {
      await session.setGroupVoice(groupId, {
        nodes: voice.nodes,
        connections: voice.connections,
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
      right = Math.max(right, position.x + NODE_WIDTH);
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
  }

  function unframeSelection() {
    for (const id of selected) {
      const node = voice.nodes[id];
      if (node) node.frame = undefined;
    }
  }

  function frameMouseDown(event: MouseEvent, frame: VoiceFrame) {
    if (event.button !== 0) return;
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
  }

  function frameResizeDown(event: MouseEvent, frame: VoiceFrame) {
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
    return (typeById.get(node.type_id)?.params ?? []).filter(
      (param) => !isLinked(node, param.name),
    );
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
      right = Math.max(right, position.x + NODE_WIDTH);
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
      const node = voice.nodes[active];
      if (node && node.type_id === "voice.group" && nodeGroup(node)) {
        void enterGroup(node);
      } else {
        void leave();
      }
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
      addMenu = null;
      nodeMenu = null;
      pendingLink = null;
      libraryOpen = false;
      interfaceOpen = false;
      sidebarOpen = false;
      return;
    }
    if (event.shiftKey && event.key.toLowerCase() === "a") {
      event.preventDefault();
      addMenu = { x: lastPointer.x, y: lastPointer.y };
      return;
    }
    if (
      event.key.toLowerCase() === "a" &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.altKey
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
    if (event.ctrlKey && !event.altKey && event.key.toLowerCase() === "p") {
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
          x: rect.width / 2 - (position[0] + NODE_WIDTH / 2) * view.scale,
          y: rect.height / 2 - (position[1] + nodeHeight(node) / 2) * view.scale,
        };
      }
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
        <button
          class="ghost small"
          disabled={selected.size === 0}
          title="Ctrl+G"
          onclick={() => void makeGroup()}
        >
          {t("voice.makeGroup")}
        </button>
        <button
          class="ghost small"
          disabled={voice.nodes[active]?.type_id !== "voice.group"}
          title="Ctrl+Alt+G"
          onclick={() => void ungroupActive()}
        >
          {t("voice.ungroup")}
        </button>
        <button class="ghost small" onclick={() => (libraryOpen = !libraryOpen)}>
          {t("voice.library")}
        </button>
        <button class="ghost small" onclick={openInterface}>
          {t("voice.interface")}
        </button>
        <button
          class="ghost small"
          title={t("graph.sidebar")}
          onclick={toggleSidebar}
        >
          N
        </button>
        <button
          class="ghost small"
          title={t("voice.close")}
          onclick={() => void leave()}
        >
          ×
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
            <span>GROUPS</span>
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

    <div class="voice-editor-main">
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div
      class="voice-editor-board"
      role="application"
      aria-label={t("voice.title")}
      bind:this={boardElement}
      oncontextmenu={(event) => {
        event.preventDefault();
        addMenu = { x: event.clientX, y: event.clientY };
      }}
      onmousedown={boardMouseDown}
      ondblclick={(event) => {
        const target = event.target as Element | null;
        if (!target?.closest(".voice-node, .node-sidebar, .node-add-menu")) {
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
      <svg class="voice-editor-wires" viewBox={`0 0 ${CANVAS_WIDTH} ${CANVAS_HEIGHT}`}>
        {#each voice.connections as connection (connection.from.node + connection.from.port + connection.to.node + connection.to.port)}
          <path d={wirePath(connection)} style:stroke={wireColor(connection)} />
        {/each}
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
        <div
          class="voice-node"
          class:selected={selected.has(node.id)}
          class:collapsed={node.collapsed}
          class:muted={node.muted}
          style:left={`${position?.x ?? 0}px`}
          style:top={`${position?.y ?? 0}px`}
          style:width={`${NODE_WIDTH}px`}
          style:height={`${nodeHeight(node)}px`}
          onmousedown={(event) => nodeMouseDown(event, node)}
          oncontextmenu={(event) => openNodeMenu(event, node)}
          ondblclick={() => void enterGroup(node)}
          role="group"
          tabindex="-1"
          data-category={typeById.get(node.type_id)?.category ?? ""}
        >
          <div class="voice-node-head">
            <span class="voice-node-title">{nodeLabel(node)}</span>
            {#if node.muted}<span class="voice-node-muted">M</span>{/if}
            <span class="voice-node-id">{node.id}</span>
          </div>

          {#if !node.collapsed}
          {#if node.type_id === "voice.group"}
            <div class="voice-node-group-line">
              <span>{nodeGroup(node)?.name ?? node.group ?? "—"}</span>
              <button
                class="voice-node-enter"
                title={t("graph.editGroup")}
                onclick={() => void enterGroup(node)}
              >
                ↗
              </button>
            </div>
          {/if}

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
              <span class="voice-socket-name in">{port.name}</span>
              {#if param && !linked}
                <div class="voice-socket-value">
                  <NumberField
                    value={node.params[param.name] ?? param.default}
                    min={param.minimum}
                    max={param.maximum}
                    step={0.01}
                    precision={2}
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
              <span class="voice-socket-name out">{port.name}</span>
              <button
                class="voice-port out"
                class:port-audio={port.ty === "audio"}
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

          {#each extraParams(node) as param, index (param.name)}
            {@const top =
              NODE_HEADER +
              groupHeaderHeight(node) +
              NODE_BODY_TOP +
              (Math.max(inputList(node).length, outputList(node).length) +
                index) *
                NODE_ROW}
            <div class="voice-socket-row param" style:top={`${top}px`}>
              <span class="voice-socket-name">{param.name}</span>
              <div class="voice-socket-value">
                <NumberField
                  value={node.params[param.name] ?? param.default}
                  min={param.minimum}
                  max={param.maximum}
                  step={0.01}
                  precision={2}
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
                class:port-float={port.ty === "float"}
                class:port-gate={port.ty === "gate"}
                style:top={`${portTop(node, index)}px`}
                title={`${port.id} (${port.ty})`}
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
                class:port-float={port.ty === "float"}
                class:port-gate={port.ty === "gate"}
                style:top={`${portTop(node, index)}px`}
                title={`${port.id} (${port.ty})`}
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
      {/each}
      </div>
      {#if marquee}
        <div class="voice-marquee" style={marqueeStyle()}></div>
      {/if}
    </div>

    {#if sidebarOpen}
      <aside class="node-sidebar" aria-label={t("graph.sidebar")}>
        <div class="node-sidebar-tabs">
          <button
            class:active={!interfaceOpen}
            onclick={() => (interfaceOpen = false)}
          >NODE</button>
          <button class:active={interfaceOpen} onclick={openInterface}>GROUP</button>
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
                    queueInterfaceSave();
                  }}
                >
                  <option value="float">FLOAT</option>
                  <option value="gate">GATE</option>
                  <option value="audio">AUDIO</option>
                </select>
                <button
                  class="ghost small"
                  onclick={() => removeSocket(section.key, index)}
                >
                  ×
                </button>
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
            <div class="node-sidebar-label">ACTIVE NODE</div>
            <strong>{nodeLabel(node)}</strong>
            <code>{node.id}</code>
            <div class="node-sidebar-type">{node.type_id}</div>
            {#if node.type_id === "voice.group"}
              <div class="node-sidebar-group">
                {nodeGroup(node)?.name ?? node.group ?? "—"}
              </div>
              <button class="ghost small" onclick={() => void enterGroup(node)}>
                {t("graph.editGroup")}
              </button>
            {/if}
            {#each sidebarParams(node) as param (param.name)}
              <label class="node-sidebar-param">
                <span>{param.name}</span>
                <NumberField
                  value={node.params[param.name] ?? param.default}
                  min={param.minimum}
                  max={param.maximum}
                  step={0.01}
                  precision={2}
                  onchange={(value) => setParam(node, param.name, value)}
                />
              </label>
            {/each}
          </div>
        {:else}
          <p class="node-sidebar-empty">No active node</p>
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
        {#if voice.nodes[nodeMenu.nodeId]?.type_id === "voice.group"}
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
          {t("graph.mute")}
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
