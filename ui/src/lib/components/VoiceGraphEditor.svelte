<script lang="ts">
  import type {
    NodeGroup,
    NodeGroupInterface,
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
  type Wire = { node: string; port: string; x: number; y: number };

  const kindLabels: Record<string, string> = {
    "group.input": "GROUP INPUT",
    "osc.partials": "OSC PARTIALS",
    "env.percussive": "ENV PERCUSSIVE",
    "noise.burst": "NOISE BURST",
    "voice.mix": "VOICE MIX",
    "voice.group": "VOICE GROUP",
    "group.output": "GROUP OUTPUT",
  };

  function nodeGroup(node: VoiceNode): NodeGroup | undefined {
    return node.group ? groups[node.group] : undefined;
  }

  function inputList(node: VoiceNode): string[] {
    if (node.kind === "group.input") return [];
    if (node.kind === "group.output") {
      return groupInterface.outputs.map((socket) => socket.id);
    }
    if (node.kind === "voice.group") {
      return nodeGroup(node)?.interface.inputs.map((socket) => socket.id) ?? [];
    }
    if (node.kind === "env.percussive") return ["in"];
    if (node.kind === "voice.mix") return ["in1", "in2"];
    return [];
  }

  function outputList(node: VoiceNode): string[] {
    if (node.kind === "group.output") return [];
    if (node.kind === "group.input") {
      return groupInterface.inputs.map((socket) => socket.id);
    }
    if (node.kind === "voice.group") {
      return nodeGroup(node)?.interface.outputs.map((socket) => socket.id) ?? [];
    }
    if (
      node.kind === "osc.partials" ||
      node.kind === "env.percussive" ||
      node.kind === "noise.burst" ||
      node.kind === "voice.mix"
    ) {
      return ["out"];
    }
    return [];
  }

  // 编辑器只在打开时创建，初始快照之后由本地编辑状态接管。
  // svelte-ignore state_referenced_locally
  let voice = $state<VoiceGraph>(
    JSON.parse(JSON.stringify(initialVoice)) as VoiceGraph,
  );
  let selected = $state("");
  let drag = $state<Drag | null>(null);
  let wire = $state<Wire | null>(null);
  let error = $state("");
  let saving = $state(false);
  let newGroupId = $state("");
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
    const outIndex = Math.max(0, outputs.indexOf(connection.from.port));
    const inIndex = Math.max(0, inputs.indexOf(connection.to.port));
    const x1 = from.x + NODE_WIDTH;
    const y1 = from.y + portY("", outIndex);
    const x2 = to.x;
    const y2 = to.y + portY("", inIndex);
    const bend = Math.max(40, Math.abs(x2 - x1) * 0.45);
    return `M ${x1} ${y1} C ${x1 + bend} ${y1}, ${x2 - bend} ${y2}, ${x2} ${y2}`;
  }

  function startWire(event: MouseEvent, node: VoiceNode, port: string) {
    event.preventDefault();
    event.stopPropagation();
    selected = node.id;
    wire = { node: node.id, port, x: event.clientX, y: event.clientY };
  }

  function finishWire(event: MouseEvent, node: VoiceNode, port: string) {
    event.preventDefault();
    event.stopPropagation();
    if (!wire) return;
    if (wire.node === node.id) {
      wire = null;
      return;
    }
    const exists = voice.connections.some(
      (connection) =>
        connection.to.node === node.id && connection.to.port === port,
    );
    voice.connections = voice.connections.filter(
      (connection) =>
        !(connection.to.node === node.id && connection.to.port === port),
    );
    if (!exists) {
      voice.connections.push({
        from: { node: wire.node, port: wire.port },
        to: { node: node.id, port },
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

  function addNode(kind: string) {
    const id = `voice-${Date.now().toString(36)}`;
    voice.nodes[id] = {
      id,
      kind,
      params: {},
      position: [80, 80],
    };
    selected = id;
  }

  function addGroupNode() {
    if (!newGroupId) return;
    const id = `voice-${Date.now().toString(36)}`;
    voice.nodes[id] = {
      id,
      kind: "voice.group",
      params: {},
      position: [80, 80],
      group: newGroupId,
    };
    selected = id;
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
    if (event.key === "Escape") {
      void leave();
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
        <button class="ghost small" disabled={saving} onclick={() => void save()}>
          {t("voice.apply")}
        </button>
        <button class="ghost small" onclick={() => void leave()}>{t("voice.close")}</button>
      </div>
    </header>

    <div class="voice-editor-toolbar">
      <button class="ghost small" onclick={() => addNode("osc.partials")}>
        + {t("voice.osc")}
      </button>
      <button class="ghost small" onclick={() => addNode("env.percussive")}>
        + {t("voice.env")}
      </button>
      <button class="ghost small" onclick={() => addNode("noise.burst")}>
        + {t("voice.noise")}
      </button>
      <button class="ghost small" onclick={() => addNode("voice.mix")}>
        + {t("voice.mix")}
      </button>
      <button class="ghost small" onclick={() => addNode("group.output")}>
        + {t("voice.output")}
      </button>
      <select class="voice-group-select" bind:value={newGroupId} aria-label={t("voice.groupNode")}>
        <option value="">{t("voice.groupNode")}</option>
        {#each groupChoices as group (group.id)}
          <option value={group.id}>{group.name}</option>
        {/each}
      </select>
      <button class="ghost small" disabled={!newGroupId} onclick={addGroupNode}>
        + {t("voice.groupNode")}
      </button>
      <button
        class="ghost small"
        disabled={!selected}
        onclick={removeSelected}
      >
        {t("voice.delete")}
      </button>
    </div>

    {#if error}
      <p class="error-text">{error}</p>
    {/if}

    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div class="voice-editor-board">
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
          <div class="voice-node-head">{kindLabels[node.kind] ?? node.kind}</div>
          <div class="voice-node-id">{node.id}</div>
          {#if node.kind === "voice.group"}
            <div class="voice-node-group">
              {nodeGroup(node)?.name ?? node.group ?? "—"}
            </div>
            <button class="ghost small" onclick={() => void enterGroup(node)}>
              {t("graph.editGroup")}
            </button>
          {/if}

          {#each outputList(node) as port, index (port)}
            <button
              class="voice-port out"
              style:top={`${portY(node.kind, index) - 4}px`}
              aria-label={port}
              onmousedown={(event) => startWire(event, node, port)}
            ></button>
          {/each}
          {#each inputList(node) as port, index (port)}
            <button
              class="voice-port in"
              style:top={`${portY(node.kind, index) - 4}px`}
              aria-label={port}
              onmousedown={(event) => finishWire(event, node, port)}
            ></button>
          {/each}

          {#if node.kind === "osc.partials"}
            <div class="voice-node-params">
              <span>partials</span><strong>{node.params.partials ?? 10}</strong>
              <span>decay</span><strong>{node.params.decay ?? 1.2}</strong>
            </div>
          {:else if node.kind === "env.percussive"}
            <div class="voice-node-params">
              <span>attack</span><strong>{node.params.attack ?? 0.004}</strong>
              <span>decay</span><strong>{node.params.decay ?? 1.2}</strong>
            </div>
          {:else if node.kind === "noise.burst"}
            <div class="voice-node-params">
              <span>level</span><strong>{node.params.level ?? 0.2}</strong>
              <span>duration</span><strong>{node.params.duration ?? 0.008}</strong>
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
