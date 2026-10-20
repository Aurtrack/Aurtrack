<script lang="ts">
  import {
    layout,
    type LayoutNode,
    type SplitNode,
  } from "../layout.svelte";
  import LayoutNodeView from "./LayoutNodeView.svelte";
  import Pane from "./Pane.svelte";

  let { node }: { node: LayoutNode } = $props();
  let container = $state<HTMLDivElement>();

  function startResize(event: PointerEvent, splitNode: SplitNode) {
    if (event.button !== 0 || !container) return;
    event.preventDefault();
    event.stopPropagation();

    const rect = container.getBoundingClientRect();
    const move = (moveEvent: PointerEvent) => {
      if (rect.width === 0 || rect.height === 0) return;
      const ratio =
        splitNode.direction === "row"
          ? (moveEvent.clientX - rect.left) / rect.width
          : (moveEvent.clientY - rect.top) / rect.height;
      layout.setRatio(splitNode.id, ratio);
    };
    const stop = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", stop);
      window.removeEventListener("pointercancel", stop);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };

    document.body.style.cursor =
      splitNode.direction === "row" ? "col-resize" : "row-resize";
    document.body.style.userSelect = "none";
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", stop);
    window.addEventListener("pointercancel", stop);
  }
</script>

{#if node.kind === "pane"}
  <Pane leaf={node} />
{:else}
  <div
    class="layout-split"
    class:row={node.direction === "row"}
    class:column={node.direction === "column"}
    bind:this={container}
  >
    <div class="layout-slot" style={`flex: ${node.ratio} 1 0%;`}>
      <LayoutNodeView node={node.first} />
    </div>
    <div
      class="layout-splitter"
      class:vertical={node.direction === "column"}
      role="separator"
      aria-orientation={node.direction === "row" ? "vertical" : "horizontal"}
      onpointerdown={(event) => startResize(event, node)}
    ></div>
    <div class="layout-slot" style={`flex: ${1 - node.ratio} 1 0%;`}>
      <LayoutNodeView node={node.second} />
    </div>
  </div>
{/if}
