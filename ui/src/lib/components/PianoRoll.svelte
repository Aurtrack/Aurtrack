<script lang="ts">
  import { onMount } from "svelte";

  import type { Clip } from "../api";
  import { t } from "../i18n.svelte";
  import {
    drawPiano,
    hitTestPiano,
    pianoHeight,
    PIANO_HIGH,
    PIANO_LOW,
    PIANO_MAX_ROW,
    PIANO_MIN_ROW,
    type PianoDragMode,
    type PianoView,
  } from "../piano";

  interface Props {
    trackId: string | null;
    clip: Clip | null;
    busy: boolean;
    onAddNote: (
      path: string,
      input: {
        pitch: number;
        start: number;
        duration: number;
        velocity: number;
      },
    ) => void;
    onUpdateNote: (
      path: string,
      patch: { pitch?: number; start?: number; duration?: number },
    ) => void;
    onRemoveNote: (path: string) => void;
  }

  let { trackId, clip, busy, onAddNote, onUpdateNote, onRemoveNote }: Props =
    $props();

  type Preview = {
    noteId: string;
    pitch: number;
    start: number;
    duration: number;
  };

  type DragState = {
    noteId: string;
    mode: PianoDragMode;
    startX: number;
    startY: number;
    originPitch: number;
    originStart: number;
    originDuration: number;
    clipDuration: number;
    pixelsPerSecond: number;
    rowHeight: number;
  };

  const SNAP = 0.125;
  const ZOOM_LEVELS = [1, 2, 4, 8];

  let canvas = $state<HTMLCanvasElement>();
  let selectedNoteId = $state("");
  let preview = $state<Preview | null>(null);
  let drag: DragState | null = null;
  let rowHeight = $state(16);
  let zoomX = $state(1);
  let viewStart = $state(0);
  let cursor = $state("crosshair");
  let lastClipId = "";
  let displayClip = $derived(applyPreview(clip, preview));
  let visibleDuration = $derived(
    Math.max(0.0625, (clip?.duration ?? 1) / zoomX),
  );
  let view = $derived<PianoView>({
    rowHeight,
    visibleDuration,
    viewStart,
  });
  let canvasHeight = $derived(pianoHeight(rowHeight));

  $effect(() => {
    const notes = clip?.notes ?? [];
    if (notes.length === 0) {
      if (selectedNoteId !== "") selectedNoteId = "";
    } else if (!notes.some((note) => note.id === selectedNoteId)) {
      selectedNoteId = notes[0]?.id ?? "";
    }
  });

  $effect(() => {
    const id = clip?.id ?? "";
    if (id !== lastClipId) {
      lastClipId = id;
      viewStart = 0;
    }
  });

  $effect(() => {
    const maximum = Math.max(0, (clip?.duration ?? 0) - visibleDuration);
    if (viewStart > maximum) viewStart = maximum;
    if (viewStart < 0) viewStart = 0;
  });

  $effect(() => {
    if (!canvas) return;
    drawPiano(canvas, displayClip, selectedNoteId, view);
  });

  onMount(() => {
    if (!canvas) return;
    const observer = new ResizeObserver(() => {
      if (!canvas) return;
      drawPiano(canvas, displayClip, selectedNoteId, view);
    });
    observer.observe(canvas);
    return () => observer.disconnect();
  });

  function applyPreview(
    source: Clip | null,
    current: Preview | null,
  ): Clip | null {
    if (!source || !current) return source;
    return {
      ...source,
      notes: source.notes.map((note) =>
        note.id === current.noteId
          ? {
              ...note,
              pitch: current.pitch,
              start: current.start,
              duration: current.duration,
            }
          : note,
      ),
    };
  }

  function snapSeconds(value: number): number {
    return Math.round(value / SNAP) * SNAP;
  }

  function clamp(value: number, minimum: number, maximum: number): number {
    return Math.min(maximum, Math.max(minimum, value));
  }

  function notePath(noteId: string): string {
    return `/tracks/${trackId}/clips/${clip?.id}/notes/${noteId}`;
  }

  function adjustPitchZoom(delta: number) {
    rowHeight = clamp(rowHeight + delta, PIANO_MIN_ROW, PIANO_MAX_ROW);
  }

  function adjustTimeZoom(direction: number) {
    const index = ZOOM_LEVELS.indexOf(zoomX);
    const next = clamp(
      (index < 0 ? 0 : index) + direction,
      0,
      ZOOM_LEVELS.length - 1,
    );
    zoomX = ZOOM_LEVELS[next] ?? 1;
  }

  function updateCursor(event: PointerEvent) {
    if (!canvas || !clip || drag) return;
    const hit = hitTestPiano(canvas, clip, event.clientX, event.clientY, view);
    if (!hit || hit.kind === "empty") {
      cursor = "crosshair";
      return;
    }
    cursor = hit.mode === "move" ? "grab" : "ew-resize";
  }

  function handlePointerDown(event: PointerEvent) {
    if (event.button !== 0 || !canvas || !clip || !trackId) return;
    const hit = hitTestPiano(canvas, clip, event.clientX, event.clientY, view);
    if (!hit) return;

    if (hit.kind === "note") {
      const note = clip.notes.find((item) => item.id === hit.noteId);
      if (!note) return;
      selectedNoteId = note.id;
      const rect = canvas.getBoundingClientRect();
      drag = {
        noteId: note.id,
        mode: hit.mode,
        startX: event.clientX,
        startY: event.clientY,
        originPitch: note.pitch,
        originStart: note.start,
        originDuration: note.duration,
        clipDuration: clip.duration,
        pixelsPerSecond: (rect.width - 56) / visibleDuration,
        rowHeight,
      };
      preview = {
        noteId: note.id,
        pitch: note.pitch,
        start: note.start,
        duration: note.duration,
      };
      cursor = hit.mode === "move" ? "grabbing" : "ew-resize";
      canvas.setPointerCapture(event.pointerId);
      event.preventDefault();
      return;
    }

    const duration = Math.min(0.25, clip.duration);
    const start = clamp(snapSeconds(hit.time), 0, clip.duration - duration);
    onAddNote(`/tracks/${trackId}/clips/${clip.id}`, {
      pitch: hit.pitch,
      start,
      duration,
      velocity: 0.8,
    });
    event.preventDefault();
  }

  function handlePointerMove(event: PointerEvent) {
    if (!canvas || !clip) return;

    if (!drag) {
      updateCursor(event);
      return;
    }

    const deltaX = (event.clientX - drag.startX) / drag.pixelsPerSecond;
    const deltaY = (event.clientY - drag.startY) / drag.rowHeight;

    let pitch = drag.originPitch;
    let start = drag.originStart;
    let duration = drag.originDuration;

    if (drag.mode === "move") {
      start = clamp(
        snapSeconds(drag.originStart + deltaX),
        0,
        drag.clipDuration - drag.originDuration,
      );
      pitch = clamp(
        drag.originPitch - Math.round(deltaY),
        PIANO_LOW,
        PIANO_HIGH,
      );
      cursor = "grabbing";
    } else if (drag.mode === "resize-start") {
      const end = drag.originStart + drag.originDuration;
      start = clamp(snapSeconds(drag.originStart + deltaX), 0, end - SNAP);
      duration = end - start;
      cursor = "ew-resize";
    } else {
      duration = clamp(
        snapSeconds(drag.originDuration + deltaX),
        SNAP,
        drag.clipDuration - drag.originStart,
      );
      cursor = "ew-resize";
    }

    preview = { noteId: drag.noteId, pitch, start, duration };
    event.preventDefault();
  }

  function handlePointerUp(event: PointerEvent) {
    if (!canvas) return;
    if (canvas.hasPointerCapture(event.pointerId)) {
      canvas.releasePointerCapture(event.pointerId);
    }

    const origin = drag;
    if (origin && preview) {
      const changed =
        preview.pitch !== origin.originPitch ||
        Math.abs(preview.start - origin.originStart) > 1e-9 ||
        Math.abs(preview.duration - origin.originDuration) > 1e-9;
      if (changed) {
        onUpdateNote(notePath(preview.noteId), {
          pitch: preview.pitch,
          start: preview.start,
          duration: preview.duration,
        });
      }
    }

    drag = null;
    preview = null;
    cursor = "crosshair";
  }

  function handlePointerCancel() {
    drag = null;
    preview = null;
    cursor = "crosshair";
  }

  function handlePointerLeave() {
    if (!drag) cursor = "crosshair";
  }

  function handleContextMenu(event: MouseEvent) {
    event.preventDefault();
    if (!canvas || !clip || !trackId) return;
    const hit = hitTestPiano(canvas, clip, event.clientX, event.clientY, view);
    if (hit?.kind === "note") {
      onRemoveNote(notePath(hit.noteId));
    }
  }
</script>

<section class="section">
  <div class="section-head">
    <span class="index">06</span>
    <span class="title">{t("piano.title")}</span>
    <span class="rule"></span>
    <span class="meta">
      {clip
        ? t("piano.meta", { clip: clip.id, notes: clip.notes.length })
        : t("piano.empty")}
    </span>
  </div>

  {#if clip && trackId}
    <div class="piano-toolbar">
      <span class="label">{t("piano.zoomPitch")}</span>
      <button
        class="tool-button"
        disabled={rowHeight <= PIANO_MIN_ROW}
        title={t("piano.zoomOut")}
        onclick={() => adjustPitchZoom(-2)}
      >
        −
      </button>
      <span class="readout">{rowHeight}px</span>
      <button
        class="tool-button"
        disabled={rowHeight >= PIANO_MAX_ROW}
        title={t("piano.zoomIn")}
        onclick={() => adjustPitchZoom(2)}
      >
        +
      </button>

      <span class="label">{t("piano.zoomTime")}</span>
      <button
        class="tool-button"
        disabled={zoomX <= 1}
        title={t("piano.zoomOut")}
        onclick={() => adjustTimeZoom(-1)}
      >
        −
      </button>
      <span class="readout">{zoomX}×</span>
      <button
        class="tool-button"
        disabled={zoomX >= 8}
        title={t("piano.zoomIn")}
        onclick={() => adjustTimeZoom(1)}
      >
        +
      </button>

      {#if zoomX > 1}
        <input
          class="piano-scroll"
          type="range"
          min="0"
          max={Math.max(0, clip.duration - visibleDuration)}
          step="0.01"
          bind:value={viewStart}
          title={t("piano.scroll")}
        />
      {/if}
    </div>

    <div class="piano-body">
      <canvas
        class="piano-canvas"
        style={`height: ${canvasHeight}px; cursor: ${cursor};`}
        bind:this={canvas}
        onpointerdown={handlePointerDown}
        onpointermove={handlePointerMove}
        onpointerup={handlePointerUp}
        onpointercancel={handlePointerCancel}
        onpointerleave={handlePointerLeave}
        oncontextmenu={handleContextMenu}
      ></canvas>
    </div>
    <div class="section-foot">
      <span>{t("piano.hint")}</span>
      <span>{clip.id} · {clip.duration.toFixed(2)} s</span>
    </div>
  {:else}
    <p class="empty">{t("piano.empty")}</p>
  {/if}
</section>
