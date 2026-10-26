<script lang="ts">
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

  const NODE_WIDTH = 190;
  const NODE_HEIGHT = 92;
  const CANVAS_WIDTH = 1400;
  const CANVAS_HEIGHT = 620;

  type Drag = {
    id: string;
    startX: number;
    startY: number;
    originX: number;
    originY: number;
  };
  type Wire = {
    node: string;
    port: string;
    ty: PortType;
    x: number;
    y: number;
  };

  type PortType = "float" | "gate" | "audio";
  type PortInfo = { id: string; ty: PortType };

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
    return sockets.map((socket) => ({ id: socket.id, ty: socket.type }));
  }

  function typePorts(
    node: VoiceNode,
    side: "inputs" | "outputs",
  ): PortInfo[] {
    return (typeById.get(node.type_id)?.[side] ?? []).map((socket) => ({
      id: socket.id,
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
  const interfaceSides = [
    { key: "inputs" as const, label: "INPUTS" },
    { key: "outputs" as const, label: "OUTPUTS" },
  ];
  let selected = $state("");
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

  function portY(kind: string, index: number): number {
    return 34 + index * 24;
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
    const y1 = from.y + portY("", outIndex);
    const x2 = to.x;
    const y2 = to.y + portY("", inIndex);
    const bend = Math.max(40, Math.abs(x2 - x1) * 0.45);
    return `M ${x1} ${y1} C ${x1 + bend} ${y1}, ${x2 - bend} ${y2}, ${x2} ${y2}`;
  }

  function startWire(
    event: MouseEvent,
    node: VoiceNode,
    port: PortInfo,
  ) {
    event.preventDefault();
    event.stopPropagation();
    selected = node.id;
    wire = {
      node: node.id,
      port: port.id,
      ty: port.ty,
      x: event.clientX,
      y: event.clientY,
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
    selected = node.id;
    const position = positions[node.id] ?? { x: 40, y: 40 };
    drag = {
      id: node.id,
      startX: event.clientX,
      startY: event.clientY,
      originX: position.x,
      originY: position.y,
    };
  }

  function move(event: MouseEvent) {
    lastPointer = { x: event.clientX, y: event.clientY };
    if (wire) {
      wire = { ...wire, x: event.clientX, y: event.clientY };
      return;
    }
    if (!drag) return;
    const node = voice.nodes[drag.id];
    if (!node) return;
    node.position = [
      drag.originX + event.clientX - drag.startX,
      drag.originY + event.clientY - drag.startY,
    ];
  }

  function end() {
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
    const rect = boardElement.getBoundingClientRect();
    return [
      Math.max(10, addMenu.x - rect.left + boardElement.scrollLeft - 20),
      Math.max(10, addMenu.y - rect.top + boardElement.scrollTop - 10),
    ];
  }

  function addNode(kind: string) {
    const id = `voice-${Date.now().toString(36)}`;
    voice.nodes[id] = {
      id,
      type_id: kind,
      params: {},
      position: addPosition(),
    };
    selected = id;
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
    selected = id;
    addMenu = null;
  }

  function removeSelected() {
    if (!selected || selected === "voice-in" || selected === "out") return;
    delete voice.nodes[selected];
    voice.connections = voice.connections.filter(
      (connection) =>
        connection.from.node !== selected && connection.to.node !== selected,
    );
    selected = "";
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
    if (event.key === "Escape" && addMenu) {
      addMenu = null;
      return;
    }
    if (event.key === "Escape") {
      void leave();
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
        <span>{t("voice.hint")}</span>
      </div>
      <div class="voice-editor-actions">
        <button class="ghost small" onclick={() => (libraryOpen = !libraryOpen)}>
          {t("voice.library")}
        </button>
        <button class="ghost small" onclick={openInterface}>
          {t("voice.interface")}
        </button>
        <button class="ghost small" disabled={saving} onclick={() => void save()}>
          {t("voice.apply")}
        </button>
        <button class="ghost small" onclick={() => void leave()}>{t("voice.close")}</button>
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
      onmousedown={(event) => {
        const target = event.target as Element | null;
        if (!target?.closest(".voice-node, .voice-add-menu")) addMenu = null;
      }}
    >
      <svg class="voice-editor-wires" viewBox={`0 0 ${CANVAS_WIDTH} ${CANVAS_HEIGHT}`}>
        {#each voice.connections as connection (connection.from.node + connection.from.port + connection.to.node + connection.to.port)}
          <path d={wirePath(connection)} />
        {/each}
      </svg>

      {#each nodes as node (node.id)}
        {@const position = positions[node.id]}
        <div
          class="voice-node"
          class:selected={selected === node.id}
          style:left={`${position?.x ?? 0}px`}
          style:top={`${position?.y ?? 0}px`}
          style:width={`${NODE_WIDTH}px`}
          style:min-height={`${NODE_HEIGHT}px`}
          onmousedown={(event) => nodeMouseDown(event, node)}
          ondblclick={() => void enterGroup(node)}
          role="group"
          tabindex="-1"
        >
          <div class="voice-node-head">{nodeLabel(node)}</div>
          <div class="voice-node-id">{node.id}</div>
          {#if node.type_id === "voice.group"}
            <div class="voice-node-group">
              {nodeGroup(node)?.name ?? node.group ?? "—"}
            </div>
            <button class="ghost small" onclick={() => void enterGroup(node)}>
              {t("graph.editGroup")}
            </button>
          {/if}

          {#each outputList(node) as port, index (port.id)}
            <button
              class="voice-port out"
              class:port-audio={port.ty === "audio"}
              class:port-float={port.ty === "float"}
              class:port-gate={port.ty === "gate"}
              style:top={`${portY(node.type_id, index) - 4}px`}
              title={`${port.id} (${port.ty})`}
              aria-label={port.id}
              onmousedown={(event) => startWire(event, node, port)}
            ></button>
          {/each}
          {#each inputList(node) as port, index (port.id)}
            <button
              class="voice-port in"
              class:port-audio={port.ty === "audio"}
              class:port-float={port.ty === "float"}
              class:port-gate={port.ty === "gate"}
              style:top={`${portY(node.type_id, index) - 4}px`}
              title={`${port.id} (${port.ty})`}
              aria-label={port.id}
              onmousedown={(event) => finishWire(event, node, port)}
            ></button>
          {/each}

          {#if typeById.get(node.type_id)?.params.length}
            <div class="voice-node-params">
              {#each typeById.get(node.type_id)?.params ?? [] as param (param.name)}
                <span>{param.name}</span>
                <input
                  class="voice-param-input"
                  type="number"
                  min={param.minimum}
                  max={param.maximum}
                  step="0.01"
                  value={node.params[param.name] ?? param.default}
                  onchange={(event) =>
                    setParam(node, param.name, Number(event.currentTarget.value))}
                />
              {/each}
            </div>
          {/if}
        </div>
      {/each}
    </div>

    {#if wire}
      <div class="voice-editor-tip">{t("voice.connecting")}</div>
    {/if}
  </section>
</div>
