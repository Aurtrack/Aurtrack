<script lang="ts">
  import { t } from "../i18n.svelte";

  interface Props {
    playing: boolean;
    busy: boolean;
    gain: number;
    status: string;
    commandCount: number;
    onPlay: () => void;
    onStop: () => void;
    onGainChange: (value: number) => void;
    onRender: () => void;
    onUndo: () => void;
    onRedo: () => void;
  }

  let {
    playing,
    busy,
    gain,
    status,
    commandCount,
    onPlay,
    onStop,
    onGainChange,
    onRender,
    onUndo,
    onRedo,
  }: Props = $props();
</script>

<section class="section">
  <div class="section-head">
    <span class="index">04</span>
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
    <button disabled={busy} onclick={onRender}>
      {t("transport.render")}
    </button>
    <button disabled={busy} onclick={onUndo}>
      {t("transport.undo")}
    </button>
    <button disabled={busy} onclick={onRedo}>
      {t("transport.redo")}
    </button>
  </div>

  <p class="status">
    {t("transport.status", { status })}
  </p>
</section>
