<script lang="ts">
  import { onMount } from "svelte";

  import { formatNumber, t } from "../i18n.svelte";
  import {
    drawTimeline,
    hitTestTimeline,
    secondsAtX,
    trackAtY,
    type TimelineHit,
    type TimelineTrack,
  } from "../timeline";

  interface Props {
    tracks: TimelineTrack[];
    duration: number;
    playhead: number;
    barSeconds: number;
    beatsPerBar: number;
    loopStart: number;
    loopEnd: number;
    onSeek?: (seconds: number) => void;
    onSelectClip?: (trackId: string, clipId: string) => void;
    onUpdateClip?: (
      path: string,
      patch: { start: number; duration: number },
    ) => void;
    onUpdateNote?: (
      path: string,
      patch: { start: number; duration: number },
    ) => void;
    onMoveClip?: (path: string, trackId: string, start: number) => void;
  }

  let {
    tracks,
    duration,
    playhead,
    barSeconds,
    beatsPerBar,
    loopStart,
    loopEnd,
    onSeek,
    onSelectClip,
    onUpdateClip,
    onUpdateNote,
    onMoveClip,
  }: Props = $props();

  type Preview = {
    kind: "clip" | "note";
    trackId: string;
    targetTrackId: string;
    clipId: string;
    noteId?: string;
    start: number;
    duration: number;
  };

  type DragState = {
    hit: TimelineHit;
    startX: number;
    originStart: number;
    originDuration: number;
    clipDuration: number;
    minDuration: number;
    pixelsPerSecond: number;
  };

  const SNAP = 0.125;

  let canvas = $state<HTMLCanvasElement>();
  let dragging = $state(false);
  let cursor = $state("crosshair");
  let preview = $state<Preview | null>(null);
  let drag: DragState | null = null;
  let height = $derived(
    Math.max(160, 54 + Math.max(1, tracks.length) * 56),
  );
  let displayTracks = $derived(applyPreview(tracks, preview));

  $effect(() => {
    if (!canvas) return;
    drawTimeline(canvas, {
      tracks: displayTracks,
      duration,
      playhead,
      empty: t("timeline.empty"),
      barSeconds,
      beatsPerBar,
      loopStart,
      loopEnd,
    });
  });

  onMount(() => {
    if (!canvas) return;
    const observer = new ResizeObserver(() => {
      if (!canvas) return;
      drawTimeline(canvas, {
        tracks: displayTracks,
        duration,
        playhead,
        empty: t("timeline.empty"),
        barSeconds,
        beatsPerBar,
        loopStart,
        loopEnd,
      });
    });
    observer.observe(canvas);
    return () => observer.disconnect();
  });

  function applyPreview(
    list: TimelineTrack[],
    current: Preview | null,
  ): TimelineTrack[] {
    if (!current) return list;
    if (current.kind === "clip" && current.targetTrackId !== current.trackId) {
      const clip = list
        .find((track) => track.id === current.trackId)
        ?.clips.find((item) => item.id === current.clipId);
      if (!clip) return list;
      return list.map((track) => {
        if (track.id === current.trackId) {
          return {
            ...track,
            clips: track.clips.filter((item) => item.id !== current.clipId),
          };
        }
        if (track.id === current.targetTrackId) {
          return {
            ...track,
            clips: [
              ...track.clips,
              {
                ...clip,
                start: current.start,
                duration: current.duration,
              },
            ],
          };
        }
        return track;
      });
    }
    return list.map((track) => {
      if (track.id !== current.targetTrackId) return track;
      return {
        ...track,
        clips: track.clips.map((clip) => {
          if (clip.id !== current.clipId) return clip;
          if (current.kind === "clip") {
            return {
              ...clip,
              start: current.start,
              duration: current.duration,
            };
          }
          return {
            ...clip,
            notes: clip.notes.map((note) =>
              note.id === current.noteId
                ? {
                    ...note,
                    start: current.start,
                    duration: current.duration,
                  }
                : note,
            ),
          };
        }),
      };
    });
  }

  function snapSeconds(value: number): number {
    return Math.round(value / SNAP) * SNAP;
  }

  function clamp(value: number, minimum: number, maximum: number): number {
    return Math.min(maximum, Math.max(minimum, value));
  }

  function handlePointerDown(event: PointerEvent) {
    if (!canvas) return;
    const hit = hitTestTimeline(
      canvas,
      {
        tracks,
        duration,
        playhead,
        empty: t("timeline.empty"),
        barSeconds,
        beatsPerBar,
        loopStart,
        loopEnd,
      },
      event.clientX,
      event.clientY,
    );

    if (hit) {
      const track = tracks.find((item) => item.id === hit.trackId);
      const clip = track?.clips.find((item) => item.id === hit.clipId);
      if (!clip) return;
      cursor = hit.mode === "move" ? "grab" : "ew-resize";
      const note =
        hit.kind === "note"
          ? clip.notes.find((item) => item.id === hit.noteId)
          : undefined;
      if (hit.kind === "note" && !note) return;

      const originStart =
        hit.kind === "clip" ? clip.start : (note?.start ?? 0);
      const originDuration =
        hit.kind === "clip" ? clip.duration : (note?.duration ?? SNAP);
      const minDuration =
        hit.kind === "clip"
          ? Math.max(
              SNAP,
              ...clip.notes.map((item) => item.start + item.duration),
            )
          : SNAP;
      const rect = canvas.getBoundingClientRect();
      onSelectClip?.(hit.trackId, hit.clipId);
      drag = {
        hit,
        startX: event.clientX,
        originStart,
        originDuration,
        clipDuration: clip.duration,
        minDuration,
        pixelsPerSecond: (rect.width - 28) / Math.max(0.001, duration),
      };
      preview = {
        kind: hit.kind,
        trackId: hit.trackId,
        targetTrackId: hit.trackId,
        clipId: hit.clipId,
        noteId: hit.kind === "note" ? hit.noteId : undefined,
        start: originStart,
        duration: originDuration,
      };
      dragging = false;
      canvas.setPointerCapture(event.pointerId);
      event.preventDefault();
      return;
    }

    dragging = true;
    cursor = "crosshair";
    preview = null;
    canvas.setPointerCapture(event.pointerId);
    onSeek?.(secondsAtX(canvas, event.clientX, duration));
  }

  function handlePointerMove(event: PointerEvent) {
    if (!canvas) return;

    if (drag) {
      const delta = (event.clientX - drag.startX) / drag.pixelsPerSecond;
      const hit = drag.hit;
      let start = drag.originStart;
      let nextDuration = drag.originDuration;

      if (hit.kind === "clip") {
        const end = drag.originStart + drag.originDuration;
        if (hit.mode === "move") {
          start = clamp(
            snapSeconds(drag.originStart + delta),
            0,
            duration - drag.originDuration,
          );
        } else if (hit.mode === "resize-start") {
          start = clamp(
            snapSeconds(drag.originStart + delta),
            0,
            end - drag.minDuration,
          );
          nextDuration = end - start;
        } else {
          nextDuration = clamp(
            snapSeconds(drag.originDuration + delta),
            drag.minDuration,
            duration - drag.originStart,
          );
        }
        let targetTrackId = hit.trackId;
        if (hit.mode === "move") {
          const index = trackAtY(canvas, tracks.length, event.clientY);
          const target = index === null ? undefined : tracks[index];
          if (target) targetTrackId = target.id;
        }
        preview = {
          kind: "clip",
          trackId: hit.trackId,
          targetTrackId,
          clipId: hit.clipId,
          start,
          duration: nextDuration,
        };
      } else {
        const end = drag.originStart + drag.originDuration;
        if (hit.mode === "move") {
          start = clamp(
            snapSeconds(drag.originStart + delta),
            0,
            drag.clipDuration - drag.originDuration,
          );
        } else if (hit.mode === "resize-start") {
          start = clamp(snapSeconds(drag.originStart + delta), 0, end - SNAP);
          nextDuration = end - start;
        } else {
          nextDuration = clamp(
            snapSeconds(drag.originDuration + delta),
            SNAP,
            drag.clipDuration - drag.originStart,
          );
        }
        preview = {
          kind: "note",
          trackId: hit.trackId,
          targetTrackId: hit.trackId,
          clipId: hit.clipId,
          noteId: hit.noteId,
          start,
          duration: nextDuration,
        };
      }

      cursor = hit.mode === "move" ? "grabbing" : "ew-resize";
      event.preventDefault();
      return;
    }

    if (dragging) {
      cursor = "crosshair";
      onSeek?.(secondsAtX(canvas, event.clientX, duration));
      return;
    }

    const hover = hitTestTimeline(
      canvas,
      {
        tracks,
        duration,
        playhead,
        empty: t("timeline.empty"),
        barSeconds,
        beatsPerBar,
        loopStart,
        loopEnd,
      },
      event.clientX,
      event.clientY,
    );
    cursor = hover
      ? hover.mode === "move"
        ? "grab"
        : "ew-resize"
      : "crosshair";
  }

  function handlePointerUp(event: PointerEvent) {
    if (!canvas) return;
    if (canvas.hasPointerCapture(event.pointerId)) {
      canvas.releasePointerCapture(event.pointerId);
    }

    const origin = drag;
    if (origin && preview) {
      const changed =
        Math.abs(preview.start - origin.originStart) > 1e-9 ||
        Math.abs(preview.duration - origin.originDuration) > 1e-9;
      const hit = origin.hit;
      const trackChanged =
        hit.kind === "clip" && preview.targetTrackId !== hit.trackId;
      if (trackChanged) {
        onMoveClip?.(
          `/tracks/${hit.trackId}/clips/${hit.clipId}`,
          preview.targetTrackId,
          preview.start,
        );
      } else if (changed) {
        if (hit.kind === "clip") {
          onUpdateClip?.(`/tracks/${hit.trackId}/clips/${hit.clipId}`, {
            start: preview.start,
            duration: preview.duration,
          });
        } else {
          onUpdateNote?.(
            `/tracks/${hit.trackId}/clips/${hit.clipId}/notes/${hit.noteId}`,
            { start: preview.start, duration: preview.duration },
          );
        }
      }
    } else if (dragging) {
      onSeek?.(secondsAtX(canvas, event.clientX, duration));
    }

    drag = null;
    preview = null;
    dragging = false;
    cursor = "crosshair";
  }

  function handlePointerCancel() {
    drag = null;
    preview = null;
    dragging = false;
    cursor = "crosshair";
  }

  function handlePointerLeave() {
    if (!drag) cursor = "crosshair";
  }
</script>

<section class="section">
  <div class="section-head">
    <span class="index">01</span>
    <span class="title">{t("timeline.title")}</span>
    <span class="rule"></span>
    <span class="meta">
      {t("timeline.meta", {
        tracks: tracks.length,
        duration: formatNumber(duration, 2),
      })}
    </span>
  </div>
  <div class="timeline-body">
    <canvas
      class="timeline-canvas"
      style={`height: ${height}px; cursor: ${cursor};`}
      bind:this={canvas}
      onpointerdown={handlePointerDown}
      onpointermove={handlePointerMove}
      onpointerup={handlePointerUp}
      onpointercancel={handlePointerCancel}
      onpointerleave={handlePointerLeave}
    ></canvas>
  </div>
  <div class="section-foot">
    <span>{t("timeline.hint")}</span>
    <span>{t("timeline.playhead", { seconds: formatNumber(playhead, 2) })}</span>
  </div>
</section>
