<script lang="ts">
  interface Props {
    lines: string[];
  }

  let { lines }: Props = $props();
</script>

<section class="section log-section">
  <div class="section-head">
    <span class="index">03</span>
    <span class="title">COMMAND LOG</span>
    <span class="rule"></span>
    <span class="meta">{lines.length} ENTRIES</span>
  </div>
  <pre class="log">{lines.join("\n")}</pre>
</section>
