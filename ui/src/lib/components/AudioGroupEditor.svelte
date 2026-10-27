<script lang="ts">
  import NodeGraph from "./NodeGraph.svelte";
  import type {
    AudioGroup,
    GraphNode,
    NodeTypeInfo,
    Track,
    TrackGraph,
    VoiceFrame,
  } from "../api";
  import { t } from "../i18n.svelte";
  import { session } from "../session.svelte";

  interface Props {
    groupId: string;
    path: { id: string; name: string }[];
    onClose: () => void;
    onNavigate: (id: string, name: string) => void;
  }

  let { groupId, path, onClose, onNavigate }: Props = $props();

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

  function openNested(node: GraphNode): void {
    if (!node.group) return;
    const nested = session.audioGroups[node.group];
    if (!nested) return;
    onNavigate(nested.id, nested.name);
  }
</script>

<div class="audio-group-editor">
  <header class="audio-group-editor-head">
    <div class="audio-group-breadcrumb">
      <button class="audio-group-root" onclick={onClose}>
        {t("graph.nodeGraph")}
      </button>
      {#each path as item (item.id)}
        <span class="audio-group-sep">/</span>
        <span class="audio-group-crumb">{item.name}</span>
      {/each}
    </div>
    <button
      class="audio-group-close"
      title={t("graph.closeGroup")}
      onclick={onClose}
    >
      ×
    </button>
  </header>
  <div class="audio-group-editor-body">
    <NodeGraph
      track={fakeTrack}
      busy={session.busy}
      compact
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
      isNodeLocked={(node) =>
        node.type_id === "audio.group.input" ||
        node.type_id === "audio.group.output"}
    />
  </div>
</div>
