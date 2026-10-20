<script lang="ts">
  import { onMount } from "svelte";

  import type { Clip } from "../api";
  import { t } from "../i18n.svelte";
  import {
    drawPiano,
    hitTestPiano,
    PIANO_HEIGHT,
    PIANO_HIGH,
    PIANO_LOW,
    PIANO_ROW,
    type PianoDragMode,
    type PianoHit,
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
  };

  const SNAP = 0.125;

  let canvas = $state<HTMLCanvasElement>();
  let selectedNoteId = $state("");
  let preview = $state<Preview | null>(null);
  let drag: DragState | null = null;
  let displayClip = $derived(applyPreview(clip, preview));

  $effect(() => {
    const notes = clip?.notes ?? [];
    if (notes.length === 0) {
      if (selectedNoteId !== "") selectedNoteId = "";
    } else if (!notes.some((note) => note.id === selectedNoteId)) {
      selectedNoteId = notes[0]?.id ?? "";
    }
  });

  $effect(() => {
    if (!canvas) return;
    drawPiano(canvas, displayClip, selectedNoteId);
  });

  onMount(() => {
    if (!canvas) return;
    const observer = new ResizeObserver(() => {
      if (!canvas) return;
      drawPiano(canvas, displayClip, selectedNoteId);
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

  function handlePointerDown(event: PointerEvent) {
    if (event.button !== 0 || !canvas || !clip || !trackId) return;
    const hit = hitTestPiano(canvas, clip, event.clientX, event.clientY);
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
        pixelsPerSecond:
          (rect.width - 56) / Math.max(0.001, clip.duration),
      };
      preview = {
        noteId: note.id,
        pitch: note.pitch,
        start: note.start,
        duration: note.duration,
      };
      canvas.setPointerCapture(event.pointerId);
      event.preventDefault();
      return;
    }

    const duration = Math.min(0.25, clip.duration);
    const start = clamp(
      snapSeconds(hit.time),
      0,
      clip.duration - duration,
    );
    onAddNote(`/tracks/${trackId}/clips/${clip.id}`, {
      pitch: hit.pitch,
      start,
      duration,
      velocity: 0.8,
    });
    event.preventDefault();
  }

  function handlePointerMove(event: PointerEvent) {
    if (!canvas || !drag) return;
    const deltaX = (event.clientX - drag.startX) / drag.pixelsPerSecond;
    const deltaY = (event.clientY - drag.startY) / PIANO_ROW;

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
    } else if (drag.mode === "resize-start") {
      const end = drag.originStart + drag.originDuration;
      start = clamp(snapSeconds(drag.originStart + deltaX), 0, end - SNAP);
      duration = end - start;
    } else {
      duration = clamp(
        snapSeconds(drag.originDuration + deltaX),
        SNAP,
        drag.clipDuration - drag.originStart,
      );
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
  }

  function handlePointerCancel() {
    drag = null;
    preview = null;
  }

  function handleContextMenu(event: MouseEvent) {
    if (!canvas || !clip || !trackId) return;
    const hit = hitTestPiano(canvas, clip, event.clientX, event.clientY);
    if (hit?.kind !== "note") return;
    event.preventDefault();
    onRemoveNote(notePath(hit.noteId));
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
    <div class="piano-body">
      <canvas
        class="piano-canvas"
        style={`height: ${PIANO_HEIGHT}px`}
        bind:this={canvas}
        onpointerdown={handlePointerDown}
        onpointermove={handlePointerMove}
        onpointerup={handlePointerUp}
        onpointercancel={handlePointerCancel}
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
