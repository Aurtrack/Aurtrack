<script lang="ts">
  import type { TimeSignature } from "../api";
  import { t } from "../i18n.svelte";
  import NumberField from "./NumberField.svelte";

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

  function changeSignature(event: Event) {
    const value = (event.currentTarget as HTMLSelectElement).value;
    const [numerator, denominator] = value.split("/").map(Number);
    if (numerator && denominator) {
      onTimebaseChange({
        time_signature: { numerator, denominator },
      });
    }
  }

  function changeLoopStart(bar: number) {
    onTimebaseChange({ loop_start: Math.max(0, (bar - 1) * barSeconds) });
  }

  function changeLoopEnd(bar: number) {
    onTimebaseChange({ loop_end: Math.max(0, (bar - 1) * barSeconds) });
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
      <NumberField
        value={tempo}
        min={20}
        max={300}
        step={1}
        precision={0}
        disabled={busy}
        onchange={(value) => onTimebaseChange({ tempo_bpm: value })}
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
      <NumberField
        value={lengthBars}
        min={1}
        max={512}
        step={1}
        precision={0}
        disabled={busy}
        onchange={(value) => onTimebaseChange({ length_bars: value })}
      />
    </label>
    <label class="field narrow">
      <span>{t("transport.loopStart")}</span>
      <NumberField
        value={loopStartBar}
        min={1}
        max={lengthBars + 1}
        step={0.25}
        precision={2}
        disabled={busy}
        onchange={changeLoopStart}
      />
    </label>
    <label class="field narrow">
      <span>{t("transport.loopEnd")}</span>
      <NumberField
        value={loopEndBar}
        min={1}
        max={lengthBars + 1}
        step={0.25}
        precision={2}
        disabled={busy}
        onchange={changeLoopEnd}
      />
    </label>
  </div>

  <div class="control-row">
    <span class="label">{t("transport.master")}</span>
    <NumberField
      value={Math.round(gain)}
      min={0}
      max={100}
      step={1}
      precision={0}
      suffix="%"
      disabled={busy}
      onchange={onGainChange}
    />
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
