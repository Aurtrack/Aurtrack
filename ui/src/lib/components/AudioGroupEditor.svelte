<script lang="ts">
  import NodeGraph from "./NodeGraph.svelte";
  import type {
    AudioGroup,
    GraphNode,
    NodeGroupInterface,
    NodeGroupSocket,
    NodeTypeInfo,
    Track,
    TrackGraph,
    VoiceFrame,
  } from "../api";
  import { errorMessage } from "../api";
  import { t } from "../i18n.svelte";
  import { session } from "../session.svelte";

  interface Props {
    groupId: string;
    path: { id: string; name: string }[];
    onClose: () => void;
    onNavigate: (id: string, name: string) => void;
    onNavigateTo: (index: number) => void;
    onExit: () => void;
  }

  let { groupId, path, onClose, onNavigate, onNavigateTo, onExit }: Props =
    $props();

  const EMPTY_GRAPH: TrackGraph = { nodes: {}, connections: [], frames: {} };

  function cloneGraph(graph: TrackGraph): TrackGraph {
    return structuredClone($state.snapshot(graph));
  }

  function groupGraph(id: string): TrackGraph {
    return session.audioGroups[id]?.graph ?? cloneGraph(EMPTY_GRAPH);
  }

  let groupName = $derived(session.audioGroups[groupId]?.name ?? groupId);
  let graph = $state<TrackGraph>(cloneGraph(EMPTY_GRAPH));
  let activeGroupId = $state("");
  let interfaceOpen = $state(false);
  let interfaceDraft = $state<NodeGroupInterface>({ inputs: [], outputs: [] });
  let interfaceError = $state("");
  let interfaceTimer: ReturnType<typeof setTimeout> | null = null;
  const interfaceSides = $derived([
    { key: "inputs" as const, label: t("graph.inputs") },
    { key: "outputs" as const, label: t("graph.outputs") },
  ]);
  let saveQueue = Promise.resolve();

  $effect(() => {
    if (activeGroupId === groupId) return;
    activeGroupId = groupId;
    graph = groupGraph(groupId);
  });

  let fakeTrack = $derived.by(
    () =>
      ({
        id: groupId,
        name: groupName,
        source: { kind: "silence" },
        devices: [],
        graph,
        clips: [],
      }) as Track,
  );

  function commit(next: TrackGraph): Promise<void> {
    graph = next;
    const payload = cloneGraph(next);
    const group = groupId;
    saveQueue = saveQueue
      .then(() => session.setAudioGroupGraph(group, payload))
      .then(() => undefined)
      .catch(() => {
        // 错误已经由 session 展示，保留当前编辑状态等待下一次操作。
      });
    return saveQueue;
  }

  function nextNodeId(): string {
    let number = 1;
    while (graph.nodes[`group-node-${number}`]) number += 1;
    return `group-node-${number}`;
  }

  function typeInfo(typeId: string): NodeTypeInfo | undefined {
    return session.audioNodeTypes.find((nodeType) => nodeType.id === typeId);
  }

  function nodeIdFromPath(path: string): string {
    const parts = path.split("/");
    return parts[parts.length - 1] ?? "";
  }

  async function addNode(
    _trackId: string,
    kind: string,
    x: number,
    y: number,
    group?: string,
  ): Promise<string | undefined> {
    const id = nextNodeId();
    const info = typeInfo(kind);
    const params: Record<string, number> = {};
    for (const param of info?.params ?? []) params[param.name] = param.default;
    const node: GraphNode = {
      id,
      kind,
      params,
      position: [x, y],
      ...(kind === "audio.group.instance" && group ? { group } : {}),
    };
    if (info) node.type_id = info.id;
    else if (kind === "audio.group.instance") node.type_id = kind;
    await commit({
      ...graph,
      nodes: { ...graph.nodes, [id]: node },
    });
    return id;
  }

  async function connect(
    _trackId: string,
    fromNodeId: string,
    fromPort: string,
    toNodeId: string,
    toPort: string,
  ): Promise<void> {
    const connections = graph.connections.filter(
      (connection) =>
        !(
          connection.to.node === toNodeId && connection.to.port === toPort
        ),
    );
    connections.push({
      from: { node: fromNodeId, port: fromPort },
      to: { node: toNodeId, port: toPort },
    });
    await commit({ ...graph, connections });
  }

  async function disconnect(
    _trackId: string,
    fromNodeId: string,
    fromPort: string,
    toNodeId: string,
    toPort: string,
  ): Promise<void> {
    await commit({
      ...graph,
      connections: graph.connections.filter(
        (connection) =>
          !(
            connection.from.node === fromNodeId &&
            connection.from.port === fromPort &&
            connection.to.node === toNodeId &&
            connection.to.port === toPort
          ),
      ),
    });
  }

  async function place(
    _trackId: string,
    nodes: { id: string; x: number; y: number }[],
  ): Promise<void> {
    const next = { ...graph.nodes };
    for (const position of nodes) {
      const node = next[position.id];
      if (!node) continue;
      next[position.id] = {
        ...node,
        position: [position.x, position.y],
      };
    }
    await commit({ ...graph, nodes: next });
  }

  async function changeParam(
    _trackId: string,
    nodeId: string,
    name: string,
    value: number,
  ): Promise<void> {
    const node = graph.nodes[nodeId];
    if (!node) return;
    await commit({
      ...graph,
      nodes: {
        ...graph.nodes,
        [nodeId]: { ...node, params: { ...node.params, [name]: value } },
      },
    });
  }

  async function changeFlags(
    _trackId: string,
    nodeId: string,
    patch: { muted?: boolean; collapsed?: boolean },
  ): Promise<void> {
    const node = graph.nodes[nodeId];
    if (!node) return;
    await commit({
      ...graph,
      nodes: { ...graph.nodes, [nodeId]: { ...node, ...patch } },
    });
  }

  async function setFrames(
    _trackId: string,
    frames: Record<string, VoiceFrame>,
    assignments: Record<string, string | null>,
  ): Promise<void> {
    const nodes = { ...graph.nodes };
    for (const [nodeId, frameId] of Object.entries(assignments)) {
      const node = nodes[nodeId];
      if (!node) continue;
      if (frameId) nodes[nodeId] = { ...node, frame: frameId };
      else {
        const { frame: _frame, ...rest } = node;
        nodes[nodeId] = rest;
      }
    }
    await commit({ ...graph, nodes, frames });
  }

  function removeIds(ids: string[]): void {
    if (ids.length === 0) return;
    const nodes = { ...graph.nodes };
    for (const id of ids) delete nodes[id];
    const gone = new Set(ids);
    void commit({
      ...graph,
      nodes,
      connections: graph.connections.filter(
        (connection) =>
          !gone.has(connection.from.node) && !gone.has(connection.to.node),
      ),
    });
  }

  async function duplicate(
    paths: string[],
  ): Promise<{ source: string; node_id: string }[]> {
    const ids = paths.map(nodeIdFromPath).filter((id) => Boolean(graph.nodes[id]));
    if (ids.length === 0) return [];
    const nodes = { ...graph.nodes };
    const mapping = new Map<string, string>();
    for (const id of ids) {
      let number = 1;
      while (
        nodes[`group-node-${number}`] ||
        [...mapping.values()].includes(`group-node-${number}`)
      )
        number += 1;
      const newId = `group-node-${number}`;
      const source = graph.nodes[id];
      if (!source) continue;
      mapping.set(id, newId);
      nodes[newId] = {
        ...source,
        id: newId,
        position: [
          (source.position?.[0] ?? 0) + 40,
          (source.position?.[1] ?? 0) + 40,
        ],
      };
    }
    const connections = [...graph.connections];
    for (const connection of graph.connections) {
      const from = mapping.get(connection.from.node);
      const to = mapping.get(connection.to.node);
      if (!from || !to) continue;
      connections.push({
        from: { node: from, port: connection.from.port },
        to: { node: to, port: connection.to.port },
      });
    }
    await commit({ ...graph, nodes, connections });
    return ids.map((id) => ({ source: id, node_id: mapping.get(id) ?? id }));
  }

  function openInterface(): void {
    const source = session.audioGroups[groupId]?.interface;
    interfaceDraft = source
      ? structuredClone($state.snapshot(source))
      : { inputs: [], outputs: [] };
    interfaceError = "";
    interfaceOpen = true;
  }

  function addSocket(side: "inputs" | "outputs"): void {
    const list = interfaceDraft[side];
    const prefix = side === "inputs" ? "in" : "out";
    let number = list.length + 1;
    while (list.some((socket) => socket.id === `${prefix}-${number}`)) {
      number += 1;
    }
    list.push({
      id: `${prefix}-${number}`,
      name: `${prefix === "in" ? t("graph.inputs") : t("graph.outputs")} ${number}`,
      type: "audio",
      default: 0,
    });
    queueInterfaceSave();
  }

  function removeSocket(side: "inputs" | "outputs", index: number): void {
    interfaceDraft[side].splice(index, 1);
    queueInterfaceSave();
  }

  function setSocketType(socket: NodeGroupSocket, type: string): void {
    socket.type = type as NodeGroupSocket["type"];
    if (type === "audio") {
      socket.default = undefined;
      socket.minimum = undefined;
      socket.maximum = undefined;
      socket.step = undefined;
    } else if (socket.default === undefined) {
      socket.default = 0;
    }
    queueInterfaceSave();
  }

  function queueInterfaceSave(): void {
    interfaceError = "";
    if (interfaceTimer) clearTimeout(interfaceTimer);
    interfaceTimer = setTimeout(() => {
      interfaceTimer = null;
      void saveInterface();
    }, 500);
  }

  async function saveInterface(): Promise<void> {
    try {
      await session.setAudioGroupInterface(
        groupId,
        structuredClone($state.snapshot(interfaceDraft)),
      );
    } catch (error) {
      interfaceError = errorMessage(error);
    }
  }

  async function packNested(_trackId: string, nodeIds: string[]): Promise<void> {
    await session.audioGroupFromGroupNodes(groupId, nodeIds);
    graph = groupGraph(groupId);
  }

  async function ungroupNested(_trackId: string, nodeId: string): Promise<void> {
    await session.ungroupAudioGroupNode(groupId, nodeId);
    graph = groupGraph(groupId);
  }

  function openNested(node: GraphNode): void {
    if (!node.group) return;
    const nested = session.audioGroups[node.group];
    if (!nested) return;
    onNavigate(nested.id, nested.name);
  }
</script>

<div class="audio-group-editor">
  <header class="voice-editor-head">
    <div>
      <div class="voice-editor-path">
        <button class="voice-editor-crumb" onclick={onClose}>
          {t("voice.trackPath")}
        </button>
        {#each path as item, index (item.id)}
          <b>/</b>
          <button
            class="voice-editor-crumb"
            class:current={index === path.length - 1}
            onclick={() => onNavigateTo(index)}
          >
            {item.name}
          </button>
        {/each}
      </div>
    </div>
    <div class="voice-editor-actions">
      <button
        class="ghost small"
        class:active={interfaceOpen}
        onclick={() => {
          if (interfaceOpen) interfaceOpen = false;
          else openInterface();
        }}
      >
        {t("voice.interface")}
      </button>
      <button class="ghost small" onclick={onClose}>
        {t("voice.close")}
      </button>
    </div>
  </header>
  <div class="audio-group-editor-main">
    {#if interfaceOpen}
      <aside class="audio-group-interface">
        <div class="audio-group-interface-head">
          <strong>{groupName} · {t("voice.interface")}</strong>
          <button
            class="audio-group-close"
            title={t("voice.close")}
            onclick={() => (interfaceOpen = false)}
          >
            ×
          </button>
        </div>
        {#if interfaceError}
          <p class="audio-group-interface-error">{interfaceError}</p>
        {/if}
        {#each interfaceSides as section (section.key)}
          {@const sockets = interfaceDraft[section.key]}
          <section class="audio-group-interface-section">
            <div class="audio-group-interface-title">
              <span>{section.label}</span>
              <button
                class="ghost small"
                onclick={() => addSocket(section.key)}
              >
                +
              </button>
            </div>
            {#each sockets as socket, index (socket.id)}
              <div class="audio-group-socket">
                <div class="audio-group-socket-head">
                  <code>{socket.id}</code>
                  <input
                    class="audio-group-socket-name"
                    value={socket.name}
                    oninput={(event) => {
                      socket.name = event.currentTarget.value;
                      queueInterfaceSave();
                    }}
                  />
                  <select
                    class="audio-group-socket-type"
                    value={socket.type}
                    onchange={(event) =>
                      setSocketType(socket, event.currentTarget.value)}
                  >
                    <option value="audio">Audio</option>
                    <option value="float">Float</option>
                    <option value="gate">Gate</option>
                  </select>
                  <button
                    class="audio-group-socket-remove"
                    title={t("voice.delete")}
                    onclick={() => removeSocket(section.key, index)}
                  >
                    ×
                  </button>
                </div>
                <div class="audio-group-socket-fields">
                  {#if socket.type !== "audio"}
                    <label>
                      <span>{t("voice.socketDefault")}</span>
                      <input
                        type="number"
                        value={socket.default ?? 0}
                        oninput={(event) => {
                          socket.default = Number(event.currentTarget.value);
                          queueInterfaceSave();
                        }}
                      />
                    </label>
                    <label>
                      <span>{t("voice.socketMinimum")}</span>
                      <input
                        type="number"
                        value={socket.minimum ?? 0}
                        oninput={(event) => {
                          socket.minimum = Number(event.currentTarget.value);
                          queueInterfaceSave();
                        }}
                      />
                    </label>
                    <label>
                      <span>{t("voice.socketMaximum")}</span>
                      <input
                        type="number"
                        value={socket.maximum ?? 1}
                        oninput={(event) => {
                          socket.maximum = Number(event.currentTarget.value);
                          queueInterfaceSave();
                        }}
                      />
                    </label>
                    <label>
                      <span>{t("voice.socketStep")}</span>
                      <input
                        type="number"
                        value={socket.step ?? 0.01}
                        oninput={(event) => {
                          socket.step = Number(event.currentTarget.value);
                          queueInterfaceSave();
                        }}
                      />
                    </label>
                  {/if}
                  <label class="wide">
                    <span>{t("voice.socketPanel")}</span>
                    <input
                      value={socket.panel ?? ""}
                      placeholder={t("voice.socketPanelHint")}
                      oninput={(event) => {
                        socket.panel =
                          event.currentTarget.value.trim() || undefined;
                        queueInterfaceSave();
                      }}
                    />
                  </label>
                  <label class="wide">
                    <span>{t("voice.socketDescription")}</span>
                    <input
                      value={socket.description ?? ""}
                      oninput={(event) => {
                        socket.description =
                          event.currentTarget.value || undefined;
                        queueInterfaceSave();
                      }}
                    />
                  </label>
                </div>
              </div>
            {/each}
          </section>
        {/each}
      </aside>
    {/if}
    <div class="audio-group-editor-body">
      <NodeGraph
        track={fakeTrack}
        busy={session.busy}
        compact
        autoFrame
        tidyOnOpen
        canExitGroup
        onExitGroup={onExit}
        onAddNode={(trackId, kind, x, y, group) =>
          addNode(trackId, kind, x, y, group)}
        onConnect={(trackId, fromNodeId, fromPort, toNodeId, toPort) =>
          connect(trackId, fromNodeId, fromPort, toNodeId, toPort)}
        onDisconnect={(trackId, fromNodeId, fromPort, toNodeId, toPort) =>
          disconnect(trackId, fromNodeId, fromPort, toNodeId, toPort)}
        onPlace={(trackId, nodes) => place(trackId, nodes)}
        onDuplicate={(paths) => duplicate(paths)}
        onRemove={(path) => removeIds([nodeIdFromPath(path)])}
        onRemoveMany={(paths) => removeIds(paths.map(nodeIdFromPath))}
        onParamChange={(trackId, nodeId, name, value) =>
          void changeParam(trackId, nodeId, name, value)}
        onFlags={(trackId, nodeId, patch) =>
          changeFlags(trackId, nodeId, patch)}
        onFrames={(trackId, frames, assignments) =>
          setFrames(trackId, frames, assignments)}
        onOpenAudioGroup={openNested}
        onMakeAudioGroup={(trackId, nodeIds) => packNested(trackId, nodeIds)}
        onUngroupAudioGroup={(trackId, nodeId) =>
          ungroupNested(trackId, nodeId)}
        isNodeLocked={(node) =>
          node.type_id === "audio.group.input" ||
          node.type_id === "audio.group.output"}
      />
    </div>
  </div>
</div>
