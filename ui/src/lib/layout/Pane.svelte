<script lang="ts">
  import { t } from "../i18n.svelte";
  import {
    layout,
    PANEL_KINDS,
    PANEL_LABEL_KEYS,
    type PanelKind,
    type PaneNode,
  } from "../layout.svelte";
  import PanelView from "./PanelView.svelte";

  let { leaf }: { leaf: PaneNode } = $props();

  function changePanel(event: Event) {
    const value = (event.currentTarget as HTMLSelectElement)
      .value as PanelKind;
    layout.setPanel(leaf.id, value);
  }
</script>

<div class="pane">
  <div class="pane-head">
    <select
      class="pane-select"
      value={leaf.panel}
      title={t("layout.panel")}
      onchange={changePanel}
    >
      {#each PANEL_KINDS as kind}
        <option value={kind}>{t(PANEL_LABEL_KEYS[kind])}</option>
      {/each}
    </select>
    <span class="pane-spacer"></span>
    <button
      class="pane-button"
      title={t("layout.splitRow")}
      onclick={() => layout.split(leaf.id, "row")}
    >
      ⇔
    </button>
    <button
      class="pane-button"
      title={t("layout.splitColumn")}
      onclick={() => layout.split(leaf.id, "column")}
    >
      ⇕
    </button>
    <button
      class="pane-button"
      title={t("layout.close")}
      disabled={!layout.canClose}
      onclick={() => layout.close(leaf.id)}
    >
      ×
    </button>
  </div>
  <div class="pane-body">
    <PanelView kind={leaf.panel} />
  </div>
</div>
