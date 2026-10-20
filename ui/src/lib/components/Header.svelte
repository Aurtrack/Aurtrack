<script lang="ts">
  import { formatNumber, t } from "../i18n.svelte";
  import MenuBar from "./MenuBar.svelte";
  import WorkspaceTabs from "./WorkspaceTabs.svelte";

  interface Props {
    audioState: "ok" | "none" | "unknown";
    sampleRate: number;
  }

  let { audioState, sampleRate }: Props = $props();
</script>

<header class="sheet-head">
  <MenuBar />
  <WorkspaceTabs />
  <div class="head-meta">
    {#if audioState === "ok"}
      <span>{t("header.audio", { rate: formatNumber(sampleRate) })}</span>
    {:else if audioState === "none"}
      <span>{t("header.noAudio")}</span>
    {:else}
      <span>{t("header.audioUnknown")}</span>
    {/if}
    <span>{t("header.build", { version: "0.0.0" })}</span>
    <span class="tag">{t("header.preAlpha")}</span>
  </div>
</header>
