<script lang="ts">
  interface Props {
    audioLabel: string;
  }

  let { audioLabel }: Props = $props();
</script>

<header class="sheet-head">
  <div class="brand">
    <div class="brand-name">AURTRACK</div>
    <div class="brand-sub">FLAT SCHEMATIC / AUDIO WORKSTATION</div>
  </div>
  <div class="head-meta">
    <span>{audioLabel}</span>
    <span>BUILD 0.0.0</span>
    <span class="tag">PRE-ALPHA</span>
  </div>
</header>
