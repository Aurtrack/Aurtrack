<script lang="ts">
  import { onMount } from "svelte";

  import Header from "./lib/components/Header.svelte";
  import LayoutNodeView from "./lib/layout/LayoutNodeView.svelte";
  import ProjectGuard from "./lib/layout/ProjectGuard.svelte";
  import { layout } from "./lib/layout.svelte";
  import { t } from "./lib/i18n.svelte";
  import { session } from "./lib/session.svelte";

  onMount(() => {
    session.start();
    return () => session.dispose();
  });

  $effect(() => {
    document.title = `${session.dirty ? "* " : ""}${
      session.projectFile || t("transport.unsaved")
    } — Aurtrack`;
  });
</script>

<main class="sheet">
  <Header audioState={session.audioState} sampleRate={session.sampleRate} />
  <div class="layout-root">
    <LayoutNodeView node={layout.root} />
  </div>
  <ProjectGuard />
</main>
