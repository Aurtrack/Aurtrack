<script lang="ts">
  import { onDestroy } from "svelte";
  import type {
    NodeGroup,
    NodeGroupInterface,
    NodeTypeInfo,
    VoiceConnection,
    VoiceGraph,
    VoiceNode,
  } from "../api";
  import { errorMessage } from "../api";
  import { t } from "../i18n.svelte";
  import { session } from "../session.svelte";
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
  let categories = $derived.by(() => {
    const groups = new Map<string, NodeTypeInfo[]>();
    for (const nodeType of nodeTypes) {
      if (nodeType.id === "group.input" || nodeType.id === "group.output") {
        continue;
      }
      if (nodeType.id === "voice.group") continue;
      const group = groups.get(nodeType.category) ?? [];
      group.push(nodeType);
      groups.set(nodeType.category, group);
    }
    return [...groups.entries()];
  });

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
  let interfaceOpen = $state(false);
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
  let wire = $state<Wire | null>(null);
  let error = $state("");
  let saving = $state(false);
  let addMenu = $state<{ x: number; y: number } | null>(null);
  let lastPointer = $state({ x: 0, y: 0 });
  let boardElement = $state<HTMLElement | null>(null);
  let groupChoices = $derived(
    Object.values(groups).filter((group) => group.id !== groupId),
  );

  let nodes = $derived(Object.values(voice.nodes));
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

  function portY(node: VoiceNode, index: number): number {
    return (
      NODE_HEADER +
      groupHeaderHeight(node) +
      NODE_BODY_TOP +
      index * NODE_ROW +
      NODE_ROW / 2
    );
  }

  function portTop(node: VoiceNode, index: number): number {
    return portY(node, index) - NODE_ROW / 2;
  }

  function nodeHeight(node: VoiceNode): number {
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
    const index = Math.max(
      0,
      outputList(node).findIndex(
        (candidate) => candidate.id === activeWire.port,
      ),
    );
    const x1 = position.x + NODE_WIDTH;
    const y1 = position.y + portY(node, index);
    const x2 = activeWire.x;
    const y2 = activeWire.y;
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
  ) {
    event.preventDefault();
    event.stopPropagation();
    selectOnly(node.id);
    const point = worldPoint(event);
    if (!point) return;
    wire = {
      node: node.id,
      port: port.id,
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
    if (!wire) return;
    if (wire.node === node.id) {
      wire = null;
      return;
    }
    if (wire.ty !== port.ty) {
      error = `端口类型不匹配：${wire.ty} → ${port.ty}`;
      wire = null;
      return;
    }
    const exists = voice.connections.some(
      (connection) =>
        connection.to.node === node.id && connection.to.port === port.id,
    );
    voice.connections = voice.connections.filter(
      (connection) =>
        !(connection.to.node === node.id && connection.to.port === port.id),
    );
    if (!exists) {
      voice.connections.push({
        from: { node: wire.node, port: wire.port },
        to: { node: node.id, port: port.id },
      });
    }
    wire = null;
  }

  function nodeMouseDown(event: MouseEvent, node: VoiceNode) {
    if (event.button !== 0) return;
    const target = event.target as Element | null;
    if (target?.closest("input, button, select, textarea")) return;
    event.preventDefault();
    const additive = event.shiftKey || event.ctrlKey || event.metaKey;
    if (additive) {
      const next = new Set(selected);
      if (next.has(node.id)) next.delete(node.id);
      else next.add(node.id);
      selected = next;
      active = node.id;
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

  function end() {
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
  });

  function openInterface() {
    interfaceDraft = JSON.parse(
      JSON.stringify(groupInterface),
    ) as NodeGroupInterface;
    interfaceError = "";
    interfaceOpen = true;
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
  }

  function removeSocket(side: "inputs" | "outputs", index: number) {
    interfaceDraft[side].splice(index, 1);
  }

  async function saveInterface() {
    interfaceError = "";
    try {
      await session.setGroupInterface(groupId, interfaceDraft);
      interfaceOpen = false;
    } catch (error) {
      interfaceError = errorMessage(error);
    }
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

  function addNode(kind: string) {
    const id = `voice-${Date.now().toString(36)}`;
    const params: Record<string, number> = {};
    for (const param of typeById.get(kind)?.params ?? []) {
      params[param.name] = param.default;
    }
    voice.nodes[id] = {
      id,
      type_id: kind,
      params,
      position: addPosition(),
    };
    selectOnly(id);
    addMenu = null;
  }

  function addGroupNode(groupId: string) {
    const id = `voice-${Date.now().toString(36)}`;
    voice.nodes[id] = {
      id,
      type_id: "voice.group",
      params: {},
      position: addPosition(),
      group: groupId,
    };
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

  async function enterGroup(node: VoiceNode) {
    const group = nodeGroup(node);
    if (!group) return;
    if (await save(false)) onEnterGroup(group.id);
  }

  async function navigateTo(index: number) {
    if (await save(false)) onNavigate(index);
  }

  async function leave() {
    if (await save(false)) onClose();
  }

  function handleKey(event: KeyboardEvent) {
    const focused = document.activeElement;
    const typing =
      focused instanceof HTMLInputElement ||
      focused instanceof HTMLTextAreaElement ||
      focused instanceof HTMLSelectElement;
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
    if (event.key === "Escape") {
      addMenu = null;
      libraryOpen = false;
      interfaceOpen = false;
      return;
    }
    if (
      event.shiftKey &&
      !event.ctrlKey &&
      !event.metaKey &&
      event.key.toLowerCase() === "a" &&
      !(document.activeElement instanceof HTMLInputElement)
    ) {
      event.preventDefault();
      addMenu = { x: lastPointer.x, y: lastPointer.y };
      return;
    }
    if (event.key === "Home") {
      event.preventDefault();
      view = { x: 0, y: 0, scale: 1 };
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
      !(document.activeElement instanceof HTMLInputElement)
    ) {
      event.preventDefault();
      removeSelected();
    }
  }
</script>

<svelte:window onmousemove={move} onmouseup={end} onkeydown={handleKey} />

<div class="voice-editor-host" class:embedded>
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
        <button class="ghost small" onclick={() => (libraryOpen = !libraryOpen)}>
          {t("voice.library")}
        </button>
        <button class="ghost small" onclick={openInterface}>
          {t("voice.interface")}
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

    {#if interfaceOpen}
      <div class="voice-interface-panel">
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
                  oninput={(event) =>
                    (socket.name = event.currentTarget.value)}
                />
                <select
                  class="voice-interface-type"
                  value={socket.type}
                  onchange={(event) => {
                    socket.type = event.currentTarget
                      .value as NodeGroupInterface["inputs"][number]["type"];
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
          <button class="ghost small" onclick={() => void saveInterface()}>
            {t("voice.apply")}
          </button>
        </div>
      </div>
    {/if}

    {#if error}
      <p class="error-text">{error}</p>
    {/if}

    {#if addMenu}
      <div
        class="voice-add-menu"
        style:left={`${addMenu.x}px`}
        style:top={`${addMenu.y}px`}
        role="menu"
        tabindex="-1"
        onmousedown={(event) => event.stopPropagation()}
      >
        {#each categories as [category, types] (category)}
          <div class="graph-menu-label">{category}</div>
          {#each types as nodeType (nodeType.id)}
            <button role="menuitem" onclick={() => addNode(nodeType.id)}>
              {nodeType.name}
            </button>
          {/each}
        {/each}
        {#if groupChoices.length > 0}
          <div class="graph-menu-label">{t("voice.groupNode")}</div>
          {#each groupChoices as group (group.id)}
            <button role="menuitem" onclick={() => addGroupNode(group.id)}>
              {group.name}
            </button>
          {/each}
        {/if}
      </div>
    {/if}

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
      onwheel={handleWheel}
      style:background-position={`${view.x}px ${view.y}px`}
      style:background-size={`${18 * view.scale}px ${18 * view.scale}px`}
    >
      <div
        class="voice-editor-canvas"
        style:transform={`translate(${view.x}px, ${view.y}px) scale(${view.scale})`}
      >
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
          style:left={`${position?.x ?? 0}px`}
          style:top={`${position?.y ?? 0}px`}
          style:width={`${NODE_WIDTH}px`}
          style:height={`${nodeHeight(node)}px`}
          onmousedown={(event) => nodeMouseDown(event, node)}
          ondblclick={() => void enterGroup(node)}
          role="group"
          tabindex="-1"
        >
          <div class="voice-node-head">
            <span class="voice-node-title">{nodeLabel(node)}</span>
            <span class="voice-node-id">{node.id}</span>
          </div>

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
                onmousedown={(event) => finishWire(event, node, port)}
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
                onmousedown={(event) => startWire(event, node, port)}
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
        </div>
      {/each}
      </div>
      {#if marquee}
        <div class="voice-marquee" style={marqueeStyle()}></div>
      {/if}
    </div>

    {#if wire}
      <div class="voice-editor-tip">{t("voice.connecting")}</div>
    {/if}
  </section>
</div>
