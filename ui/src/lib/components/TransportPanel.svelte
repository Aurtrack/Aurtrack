<script lang="ts">
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
    <span class="title">TRANSPORT</span>
    <span class="rule"></span>
    <span class="meta">{commandCount} COMMANDS</span>
  </div>

  <div class="control-row">
    <span class="label">MASTER</span>
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
    <button disabled={busy} onclick={onUndo}>UNDO</button>
    <button disabled={busy} onclick={onRedo}>REDO</button>
  </div>

  <p class="status">STATUS: {status}</p>
</section>
