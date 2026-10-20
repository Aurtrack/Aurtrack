<script lang="ts">
  import { onMount } from "svelte";

  import { drawTimeline, secondsAtX, type TimelineTrack } from "../timeline";

  interface Props {
    tracks: TimelineTrack[];
    duration: number;
    playhead: number;
    onSeek?: (seconds: number) => void;
  }

  let { tracks, duration, playhead, onSeek }: Props = $props();
  let canvas = $state<HTMLCanvasElement>();
  let dragging = $state(false);
  let height = $derived(
    Math.max(160, 54 + Math.max(1, tracks.length) * 56),
  );

  $effect(() => {
    if (!canvas) return;
    drawTimeline(canvas, { tracks, duration, playhead });
  });

  onMount(() => {
    if (!canvas) return;
    const observer = new ResizeObserver(() => {
      if (!canvas) return;
      drawTimeline(canvas, { tracks, duration, playhead });
    });
    observer.observe(canvas);
    return () => observer.disconnect();
  });

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
    <span class="meta">{tracks.length} TRACKS / 5.00 S</span>
  </div>
  <div class="timeline-body">
    <canvas
      class="timeline-canvas"
      style={`height: ${height}px`}
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
