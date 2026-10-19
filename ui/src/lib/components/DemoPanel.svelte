<script lang="ts">
  interface Props {
    gain: number;
    busy: boolean;
    playing: boolean;
    status: string;
    trackLabel: string;
    onGainChange: (value: number) => void;
    onPlay: () => void;
    onStop: () => void;
    onRender: () => void;
    onReset: () => void;
  }

  let {
    gain,
    busy,
    playing,
    status,
    trackLabel,
    onGainChange,
    onPlay,
    onStop,
    onRender,
    onReset,
  }: Props = $props();
</script>

<section class="section control-section">
  <div class="section-head">
    <span class="index">02</span>
    <span class="title">CONTROL</span>
    <span class="rule"></span>
    <span class="meta">GAIN</span>
  </div>
  <div class="control-row">
    <span class="label">TRACK</span>
    <span class="readout wide">{trackLabel}</span>
  </div>
  <div class="control-row">
    <span class="label">GAIN</span>
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
      PLAY
    </button>
    <button disabled={busy || !playing} onclick={onStop}>STOP</button>
    <button disabled={busy} onclick={onRender}>RENDER WAV</button>
    <button disabled={busy} onclick={onReset}>RESET</button>
  </div>
  <p class="status">STATUS: {status}</p>
</section>
