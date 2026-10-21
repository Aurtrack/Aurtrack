<script lang="ts">
  import type { TimeSignature } from "../api";
  import { t } from "../i18n.svelte";

  interface Props {
    playing: boolean;
    busy: boolean;
    metronome: boolean;
    gain: number;
    status: string;
    fileName: string;
    dirty: boolean;
    commandCount: number;
    tempo: number;
    timeSignature: TimeSignature;
    lengthBars: number;
    loopStart: number;
    loopEnd: number;
    barSeconds: number;
    onPlay: () => void;
    onStop: () => void;
    onGainChange: (value: number) => void;
    onMetronomeChange: (enabled: boolean) => void;
    onTimebaseChange: (patch: {
      tempo_bpm?: number;
      time_signature?: TimeSignature;
      length_bars?: number;
      loop_start?: number;
      loop_end?: number;
    }) => void;
  }

  let {
    playing,
    busy,
    metronome,
    gain,
    status,
    fileName,
    dirty,
    commandCount,
    tempo,
    timeSignature,
    lengthBars,
    loopStart,
    loopEnd,
    barSeconds,
    onPlay,
    onStop,
    onGainChange,
    onMetronomeChange,
    onTimebaseChange,
  }: Props = $props();

  let signature = $derived(
    `${timeSignature.numerator}/${timeSignature.denominator}`,
  );
  let loopStartBar = $derived(
    barSeconds > 0 ? loopStart / barSeconds + 1 : 1,
  );
  let loopEndBar = $derived(barSeconds > 0 ? loopEnd / barSeconds + 1 : 1);

  function inputValue(event: Event): number {
    return Number((event.currentTarget as HTMLInputElement).value);
  }

  function changeSignature(event: Event) {
    const value = (event.currentTarget as HTMLSelectElement).value;
    const [numerator, denominator] = value.split("/").map(Number);
    if (numerator && denominator) {
      onTimebaseChange({
        time_signature: { numerator, denominator },
      });
    }
  }

  function changeLoopStart(event: Event) {
    onTimebaseChange({
      loop_start: Math.max(0, (inputValue(event) - 1) * barSeconds),
    });
  }

  function changeLoopEnd(event: Event) {
    onTimebaseChange({
      loop_end: Math.max(0, (inputValue(event) - 1) * barSeconds),
    });
  }
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

  <div class="timebase-grid">
    <label class="field narrow">
      <span>{t("transport.tempo")}</span>
      <input
        class="text-input"
        type="number"
        min="20"
        max="300"
        step="1"
        value={tempo}
        disabled={busy}
        onchange={(event) =>
          onTimebaseChange({ tempo_bpm: inputValue(event) })}
      />
    </label>
    <label class="field narrow">
      <span>{t("transport.signature")}</span>
      <select
        class="text-input"
        value={signature}
        disabled={busy}
        onchange={changeSignature}
      >
        <option value="2/4">2/4</option>
        <option value="3/4">3/4</option>
        <option value="4/4">4/4</option>
        <option value="5/4">5/4</option>
        <option value="6/8">6/8</option>
        <option value="7/8">7/8</option>
      </select>
    </label>
    <label class="field narrow">
      <span>{t("transport.bars")}</span>
      <input
        class="text-input"
        type="number"
        min="1"
        max="512"
        step="1"
        value={lengthBars}
        disabled={busy}
        onchange={(event) =>
          onTimebaseChange({ length_bars: inputValue(event) })}
      />
    </label>
    <label class="field narrow">
      <span>{t("transport.loopStart")}</span>
      <input
        class="text-input"
        type="number"
        min="1"
        step="0.25"
        value={loopStartBar.toFixed(2)}
        disabled={busy}
        onchange={changeLoopStart}
      />
    </label>
    <label class="field narrow">
      <span>{t("transport.loopEnd")}</span>
      <input
        class="text-input"
        type="number"
        min="1"
        step="0.25"
        value={loopEndBar.toFixed(2)}
        disabled={busy}
        onchange={changeLoopEnd}
      />
    </label>
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
    <label class="metronome-toggle">
      <input
        type="checkbox"
        checked={metronome}
        disabled={busy}
        onchange={(event) =>
          onMetronomeChange(
            (event.currentTarget as HTMLInputElement).checked,
          )}
      />
      <span>{t("transport.metronome")}</span>
    </label>
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
