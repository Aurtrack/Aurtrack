<script lang="ts">
  import { t } from "../i18n.svelte";

  interface Props {
    lines: string[];
  }

  let { lines }: Props = $props();
</script>

<section class="section log-section">
  <div class="section-head">
    <span class="index">05</span>
    <span class="title">{t("log.title")}</span>
    <span class="rule"></span>
    <span class="meta">{t("log.entries", { count: lines.length })}</span>
  </div>
  <pre class="log">{lines.join("\n")}</pre>
</section>
