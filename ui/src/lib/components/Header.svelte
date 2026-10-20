<script lang="ts">
  import {
    formatNumber,
    getLocale,
    t,
    toggleLocale,
  } from "../i18n.svelte";

  interface Props {
    audioState: "ok" | "none" | "unknown";
    sampleRate: number;
  }

  let { audioState, sampleRate }: Props = $props();
  let isChinese = $derived(getLocale() === "zh-CN");
</script>

<header class="sheet-head">
  <div class="brand">
    <div class="brand-name">AURTRACK</div>
    <div class="brand-sub">{t("app.tagline")}</div>
  </div>
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
    <button
      class="lang-toggle"
      title={t("header.switchLanguage")}
      onclick={toggleLocale}
    >
      {isChinese ? "EN" : "中文"}
    </button>
  </div>
</header>
