<script lang="ts">
  import { onMount } from "svelte";

  import { drawTimeline, secondsAtX, type Clip } from "../timeline";

  interface Props {
    trackName: string;
    duration: number;
    playhead: number;
    clips: Clip[];
    onSeek?: (seconds: number) => void;
  }

  let { trackName, duration, playhead, clips, onSeek }: Props = $props();
  let canvas = $state<HTMLCanvasElement>();

  $effect(() => {
    if (!canvas) return;
    drawTimeline(canvas, { trackName, duration, playhead, clips });
  });

  onMount(() => {
    if (!canvas) return;
    const observer = new ResizeObserver(() => {
      if (!canvas) return;
      drawTimeline(canvas, { trackName, duration, playhead, clips });
    });
    observer.observe(canvas);
    return () => observer.disconnect();
  });

  let dragging = $state(false);

  function handlePointerDown(event: PointerEvent) {
    if (!canvas || !onSeek) return;
    dragging = true;
    canvas.setPointerCapture(event.pointerId);
    onSeek(secondsAtX(canvas, event.clientX, duration));
  }

  function handlePointerMove(event: PointerEvent) {
    if (!dragging || !canvas || !onSeek) return;
    onSeek(secondsAtX(canvas, event.clientX, duration));
  }

  function handlePointerUp(event: PointerEvent) {
    if (!canvas) return;
    dragging = false;
    if (canvas.hasPointerCapture(event.pointerId)) {
      canvas.releasePointerCapture(event.pointerId);
    }
    if (onSeek) {
      onSeek(secondsAtX(canvas, event.clientX, duration));
    }
  }
</script>

<section class="section">
  <div class="section-head">
    <span class="index">01</span>
    <span class="title">TIMELINE</span>
    <span class="rule"></span>
    <span class="meta">5.00 S / 48.0 KHZ</span>
  </div>
  <div class="timeline-body">
    <div class="timeline-label">
      <span>A-01</span>
      <span>{trackName.toUpperCase()}</span>
    </div>
    <canvas
      class="timeline-canvas"
      bind:this={canvas}
      onpointerdown={handlePointerDown}
      onpointermove={handlePointerMove}
      onpointerup={handlePointerUp}
      onpointercancel={handlePointerUp}
    ></canvas>
  </div>
  <div class="section-foot">
    <span>CLICK TO SEEK</span>
    <span>T+{playhead.toFixed(2)} S</span>
  </div>
</section>
