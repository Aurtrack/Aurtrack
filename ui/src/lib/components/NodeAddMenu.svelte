<script lang="ts">
  import type { AudioGroup, NodeGroup, NodeTypeInfo } from "../api";
  import { t } from "../i18n.svelte";
  import { nodeCategoryLabel, nodeTypeLabel } from "../nodeLabels";

  interface Props {
    x: number;
    y: number;
    nodeTypes: NodeTypeInfo[];
    groups?: NodeGroup[];
    groupLabel?: string;
    audioGroups?: AudioGroup[];
    audioGroupLabel?: string;
    onPickType: (typeId: string) => void;
    onPickGroup?: (groupId: string) => void;
    onPickAudioGroup?: (groupId: string) => void;
    onClose: () => void;
  }

  let {
    x,
    y,
    nodeTypes,
    groups = [],
    groupLabel = t("graph.groups"),
    audioGroups = [],
    audioGroupLabel = t("graph.audioGroups"),
    onPickType,
    onPickGroup,
    onPickAudioGroup,
    onClose,
  }: Props = $props();

  let query = $state("");
  let input = $state<HTMLInputElement | null>(null);
  let selected = $state(0);

  const categories = $derived.by(() => {
    const result = new Map<string, NodeTypeInfo[]>();
    for (const nodeType of nodeTypes) {
      const list = result.get(nodeType.category) ?? [];
      list.push(nodeType);
      result.set(nodeType.category, list);
    }
    return [...result.entries()];
  });

  const results = $derived.by(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return [];
    return nodeTypes.filter(
      (nodeType) =>
        nodeType.name.toLowerCase().includes(needle) ||
        nodeType.id.toLowerCase().includes(needle) ||
        nodeType.category.toLowerCase().includes(needle) ||
        nodeTypeLabel(nodeType).toLowerCase().includes(needle) ||
        nodeCategoryLabel(nodeType.category).toLowerCase().includes(needle),
    );
  });

  const groupResults = $derived.by(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return [];
    return groups.filter(
      (group) =>
        group.name.toLowerCase().includes(needle) ||
        group.id.toLowerCase().includes(needle),
    );
  });

  const audioGroupResults = $derived.by(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return [];
    return audioGroups.filter(
      (group) =>
        group.name.toLowerCase().includes(needle) ||
        group.id.toLowerCase().includes(needle),
    );
  });

  const flat = $derived.by(() => {
    if (!query.trim()) return [];
    return [
      ...results.map((nodeType) => ({ kind: "type" as const, nodeType })),
      ...groupResults.map((group) => ({ kind: "group" as const, group })),
      ...audioGroupResults.map((group) => ({
        kind: "audio-group" as const,
        group,
      })),
    ];
  });

  $effect(() => {
    input?.focus();
  });

  $effect(() => {
    selected = 0;
  });

  function pick(index: number) {
    const item = flat[index];
    if (!item) return;
    if (item.kind === "type") onPickType(item.nodeType.id);
    else if (item.kind === "group") onPickGroup?.(item.group.id);
    else onPickAudioGroup?.(item.group.id);
  }

  function handleKey(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
      return;
    }
    if (event.key === "ArrowDown") {
      event.preventDefault();
      selected = Math.min(flat.length - 1, selected + 1);
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      selected = Math.max(0, selected - 1);
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      if (flat.length > 0) pick(selected);
      return;
    }
  }

  function position(): string {
    const width = 232;
    const left = Math.max(8, Math.min(x, window.innerWidth - width - 8));
    const top = Math.max(8, Math.min(y, window.innerHeight - 360));
    return `left:${left}px;top:${top}px;`;
  }
</script>

<div
  class="node-add-menu"
  style={position()}
  role="menu"
  tabindex="-1"
  onmousedown={(event) => event.stopPropagation()}
>
  <input
    class="node-add-search"
    bind:this={input}
    bind:value={query}
    placeholder={t("graph.searchNodes")}
    aria-label={t("graph.searchNodes")}
    onkeydown={handleKey}
  />
  <div class="node-add-list">
    {#if query.trim()}
      {#each flat as item, index (item.kind === "type" ? item.nodeType.id : item.group.id)}
        <button
          role="menuitem"
          class:selected={index === selected}
          onclick={() => pick(index)}
        >
          <span>
            {item.kind === "type"
              ? nodeTypeLabel(item.nodeType)
              : item.group.name}
          </span>
          <small>
            {item.kind === "type"
              ? nodeCategoryLabel(item.nodeType.category)
              : item.kind === "group"
                ? groupLabel
                : audioGroupLabel}
          </small>
        </button>
      {/each}
      {#if flat.length === 0}
        <p class="node-add-empty">{t("graph.noResults")}</p>
      {/if}
    {:else}
      {#each categories as [category, types] (category)}
        <div class="node-add-label">{nodeCategoryLabel(category)}</div>
        {#each types as nodeType (nodeType.id)}
          <button role="menuitem" onclick={() => onPickType(nodeType.id)}>
            <span>{nodeTypeLabel(nodeType)}</span>
            <small>{nodeType.id}</small>
          </button>
        {/each}
      {/each}
      {#if groups.length > 0}
        <div class="node-add-label">{groupLabel}</div>
        {#each groups as group (group.id)}
          <button role="menuitem" onclick={() => onPickGroup?.(group.id)}>
            <span>{group.name}</span>
            <small>{group.id}</small>
          </button>
        {/each}
      {/if}
      {#if audioGroups.length > 0}
        <div class="node-add-label">{audioGroupLabel}</div>
        {#each audioGroups as group (group.id)}
          <button role="menuitem" onclick={() => onPickAudioGroup?.(group.id)}>
            <span>{group.name}</span>
            <small>{group.id}</small>
          </button>
        {/each}
      {/if}
    {/if}
  </div>
</div>
