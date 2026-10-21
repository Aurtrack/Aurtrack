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

  function isTypingTarget(target: EventTarget | null): boolean {
    const element = target as HTMLElement | null;
    if (!element) return false;
    return (
      element.tagName === "INPUT" ||
      element.tagName === "TEXTAREA" ||
      element.tagName === "SELECT" ||
      element.isContentEditable
    );
  }

  function isButtonTarget(target: EventTarget | null): boolean {
    const element = target as HTMLElement | null;
    return element?.tagName === "BUTTON" || element?.tagName === "A";
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.defaultPrevented || isTypingTarget(event.target)) return;
    const modifier = event.ctrlKey || event.metaKey;
    const key = event.key.toLowerCase();

    if (modifier && key === "s") {
      event.preventDefault();
      void session.saveProject();
    } else if (modifier && key === "o") {
      event.preventDefault();
      void session.openProject();
    } else if (modifier && key === "z") {
      event.preventDefault();
      if (event.shiftKey) void session.redo();
      else void session.undo();
    } else if (modifier && key === "y") {
      event.preventDefault();
      void session.redo();
    } else if (modifier && key === "c") {
      event.preventDefault();
      session.copySelected();
    } else if (modifier && key === "v") {
      event.preventDefault();
      void session.pasteClipboard();
    } else if (modifier && key === "d") {
      event.preventDefault();
      void session.duplicateSelected();
    } else if (key === "delete" || key === "backspace") {
      event.preventDefault();
      void session.deleteSelected();
    } else if (key === " " && !isButtonTarget(event.target)) {
      event.preventDefault();
      if (session.playing) void session.stop();
      else void session.play();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<main class="sheet">
  <Header audioState={session.audioState} sampleRate={session.sampleRate} />
  <div class="layout-root">
    <LayoutNodeView node={layout.root} />
  </div>
  <ProjectGuard />
</main>
