<script lang="ts">
  import { t } from "../i18n.svelte";

  interface Props {
    playing: boolean;
    busy: boolean;
    gain: number;
    status: string;
    fileName: string;
    dirty: boolean;
    commandCount: number;
    onPlay: () => void;
    onStop: () => void;
    onGainChange: (value: number) => void;
  }

  let {
    playing,
    busy,
    gain,
    status,
    fileName,
    dirty,
    commandCount,
    onPlay,
    onStop,
    onGainChange,
  }: Props = $props();
</script>

<section class="section">
  <div class="section-head">
    <span class="index">05</span>
    <span class="title">{t("transport.title")}</span>
    <span class="rule"></span>
    <span class="meta">
      {t("transport.commands", { count: commandCount })}
    </span>
  </div>

  <div class="control-row">
    <span class="label">{t("transport.master")}</span>
    <input
      type="range"
      min="0"
      max="100"
      value={gain}
      oninput={(event) =>
        onGainChange(Number((event.currentTarget as HTMLInputElement).value))}
    />
    <span class="readout">{Math.round(gain).toString().padStart(3, "0")}%</span>
  </div>

  <div class="actions">
    <button class="primary" disabled={busy || playing} onclick={onPlay}>
      {t("transport.play")}
    </button>
    <button disabled={busy || !playing} onclick={onStop}>
      {t("transport.stop")}
    </button>
  </div>

  <p class="status">
    {t("transport.file", {
      file: `${fileName || t("transport.unsaved")}${dirty ? " *" : ""}`,
    })}
  </p>
  <p class="status">
    {t("transport.status", { status })}
  </p>
</section>
