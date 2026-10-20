<script lang="ts">
  import { t } from "../i18n.svelte";
  import { layout, WORKSPACES } from "../layout.svelte";
</script>

<div class="workspace-tabs" role="tablist" aria-label={t("workspace.layout")}>
  {#each WORKSPACES as workspace}
    <button
      class="workspace-tab"
      class:active={layout.activeWorkspace === workspace.id}
      role="tab"
      aria-selected={layout.activeWorkspace === workspace.id}
      onclick={() => layout.setWorkspace(workspace.id)}
    >
      {t(workspace.label)}
    </button>
  {/each}
</div>
