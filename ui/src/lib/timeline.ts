export type TimelineNote = {
  id: string;
  pitch: number;
  start: number;
  duration: number;
};

export type TimelineClip = {
  id: string;
  name: string;
  start: number;
  duration: number;
  notes: TimelineNote[];
};

export type TimelineTrack = {
  id: string;
  name: string;
  frequency: number | null;
  meta: string;
  clips: TimelineClip[];
};

export type TimelineView = {
  tracks: TimelineTrack[];
  duration: number;
  playhead: number;
  empty: string;
  barSeconds: number;
  beatsPerBar: number;
  loopStart: number;
  loopEnd: number;
};

export type TimelineDragMode = "move" | "resize-start" | "resize-end";

export type TimelineHit =
  | {
      kind: "clip";
      trackId: string;
      clipId: string;
      mode: TimelineDragMode;
    }
  | {
      kind: "note";
      trackId: string;
      clipId: string;
      noteId: string;
      mode: TimelineDragMode;
    };

const PADDING = 14;
const RULER_HEIGHT = 26;
const LANE_GAP = 4;

const COLORS = {
  paper: "#ecece8",
  dot: "#c4cad3",
  grid: "#c3c9d1",
  line: "#98a3b2",
  blue: "#2f5fd0",
  blueDark: "#1e3f8f",
  muted: "#5f6878",
  white: "#ffffff",
};

const SANS_FONT =
  "'Bahnschrift', 'Microsoft YaHei', 'PingFang SC', 'Segoe UI', sans-serif";
const MONO_FONT =
  "'Cascadia Mono', Consolas, 'Microsoft YaHei', 'PingFang SC', monospace";

const NOTE_NAMES = [
  "C",
  "C#",
  "D",
  "D#",
  "E",
  "F",
  "F#",
  "G",
  "G#",
  "A",
  "A#",
  "B",
];

function noteName(pitch: number): string {
  const name = NOTE_NAMES[pitch % 12] ?? "?";
  return `${name}${Math.floor(pitch / 12) - 1}`;
}

export function drawTimeline(
  canvas: HTMLCanvasElement,
  view: TimelineView,
): void {
  const width = canvas.clientWidth;
  const height = canvas.clientHeight;
  if (width === 0 || height === 0 || view.duration <= 0) return;

  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const pixelWidth = Math.round(width * dpr);
  const pixelHeight = Math.round(height * dpr);
  if (canvas.width !== pixelWidth || canvas.height !== pixelHeight) {
    canvas.width = pixelWidth;
    canvas.height = pixelHeight;
  }

  const context = canvas.getContext("2d");
  if (!context) return;

  context.setTransform(dpr, 0, 0, dpr, 0, 0);
  context.clearRect(0, 0, width, height);
  context.fillStyle = COLORS.paper;
  context.fillRect(0, 0, width, height);

  context.fillStyle = COLORS.dot;
  for (let x = 10; x < width; x += 24) {
    for (let y = 10; y < height; y += 24) {
      context.fillRect(x, y, 1, 1);
    }
  }

  const laneTop = PADDING + RULER_HEIGHT + 6;
  const laneArea = height - laneTop - PADDING;
  const usableWidth = width - PADDING * 2;
  const pixelsPerSecond = usableWidth / view.duration;

  context.strokeStyle = COLORS.line;
  context.lineWidth = 1;
  context.beginPath();
  context.moveTo(PADDING, PADDING + RULER_HEIGHT);
  context.lineTo(PADDING + usableWidth, PADDING + RULER_HEIGHT);
  context.stroke();

  const barSeconds = Math.max(0.0625, view.barSeconds);
  const beatsPerBar = Math.max(1, view.beatsPerBar);
  const beatSeconds = barSeconds / beatsPerBar;
  const barCount = Math.ceil(view.duration / barSeconds - 1e-9);
  const showBeats = (beatSeconds * pixelsPerSecond) >= 10;
  context.font = `10px ${MONO_FONT}`;
  for (let bar = 0; bar <= barCount; bar += 1) {
    const barTime = bar * barSeconds;
    const x = PADDING + barTime * pixelsPerSecond;
    context.strokeStyle = COLORS.line;
    context.beginPath();
    context.moveTo(x, PADDING + 12);
    context.lineTo(x, PADDING + RULER_HEIGHT);
    context.stroke();
    context.fillStyle = COLORS.muted;
    context.fillText(`${bar + 1}`, x + 4, PADDING + 10);

    if (showBeats) {
      for (let beat = 1; beat < beatsPerBar; beat += 1) {
        const beatTime = barTime + beat * beatSeconds;
        if (beatTime > view.duration + 1e-9) break;
        const beatX = PADDING + beatTime * pixelsPerSecond;
        context.strokeStyle = COLORS.grid;
        context.beginPath();
        context.moveTo(beatX, PADDING + 20);
        context.lineTo(beatX, PADDING + RULER_HEIGHT);
        context.stroke();
      }
    }
  }

  if (view.loopEnd > view.loopStart) {
    const loopStartX = PADDING + view.loopStart * pixelsPerSecond;
    const loopEndX = PADDING + view.loopEnd * pixelsPerSecond;
    context.strokeStyle = COLORS.blue;
    context.setLineDash([4, 3]);
    context.beginPath();
    context.moveTo(loopStartX, PADDING + RULER_HEIGHT + 2);
    context.lineTo(loopStartX, laneTop + laneArea);
    context.moveTo(loopEndX, PADDING + RULER_HEIGHT + 2);
    context.lineTo(loopEndX, laneTop + laneArea);
    context.stroke();
    context.setLineDash([]);
    context.fillStyle = COLORS.blue;
    context.fillRect(loopStartX - 2, PADDING + RULER_HEIGHT - 3, 4, 4);
    context.fillRect(loopEndX - 2, PADDING + RULER_HEIGHT - 3, 4, 4);
    context.font = `8px ${MONO_FONT}`;
    context.fillText("LOOP", loopStartX + 4, PADDING + RULER_HEIGHT + 10);
  }

  if (view.tracks.length === 0) {
    context.fillStyle = COLORS.muted;
    context.font = `10px ${MONO_FONT}`;
    context.fillText(view.empty, PADDING + 4, laneTop + 18);
  } else {
    const laneHeight = laneArea / view.tracks.length;
    for (let index = 0; index < view.tracks.length; index += 1) {
      const track = view.tracks[index];
      if (!track) continue;

      const y = laneTop + index * laneHeight;
      const laneY = y + LANE_GAP / 2;
      const laneH = laneHeight - LANE_GAP;
      context.strokeStyle = COLORS.line;
      context.strokeRect(PADDING + 0.5, laneY + 0.5, usableWidth - 1, laneH - 1);

      if (track.clips.length > 0) {
        context.fillStyle = COLORS.muted;
        context.font = `9px ${MONO_FONT}`;
        context.fillText(
          `${track.name} · ${track.meta}`,
          PADDING + 4,
          laneY + 10,
        );

        const clipTop = laneY + 16;
        const clipHeight = Math.max(18, laneH - 20);
        for (const clip of track.clips) {
          const clipX = PADDING + clip.start * pixelsPerSecond;
          const clipW = Math.max(2, clip.duration * pixelsPerSecond);
          context.fillStyle = COLORS.blue;
          context.fillRect(clipX, clipTop, clipW, clipHeight);
          context.strokeStyle = COLORS.blueDark;
          context.strokeRect(
            clipX + 0.5,
            clipTop + 0.5,
            clipW - 1,
            clipHeight - 1,
          );

          context.save();
          context.beginPath();
          context.rect(clipX + 1, clipTop, Math.max(0, clipW - 2), clipHeight);
          context.clip();

          context.fillStyle = COLORS.white;
          context.font = `9px ${SANS_FONT}`;
          context.fillText(clip.name, clipX + 5, clipTop + 11);

          for (const note of clip.notes) {
            const noteX = clipX + note.start * pixelsPerSecond;
            const noteW = Math.max(2, note.duration * pixelsPerSecond);
            const noteY = clipTop + clipHeight - 8;
            context.fillStyle = COLORS.white;
            context.fillRect(noteX, noteY, noteW, 3);
            if (noteW > 18) {
              context.font = `8px ${MONO_FONT}`;
              context.fillText(noteName(note.pitch), noteX + 1, noteY - 2);
            }
          }
          context.restore();
        }
      } else if (track.frequency === null) {
        context.fillStyle = COLORS.muted;
        context.font = `11px ${SANS_FONT}`;
        context.fillText(track.name, PADDING + 10, laneY + 18);
        context.font = `9px ${MONO_FONT}`;
        context.fillText(track.meta, PADDING + 10, laneY + 32);
      } else {
        const blockY = laneY + 5;
        const blockH = Math.max(22, laneH - 10);
        context.fillStyle = COLORS.blue;
        context.fillRect(PADDING, blockY, usableWidth, blockH);
        context.strokeStyle = COLORS.blueDark;
        context.strokeRect(
          PADDING + 0.5,
          blockY + 0.5,
          usableWidth - 1,
          blockH - 1,
        );

        context.fillStyle = COLORS.white;
        context.fillRect(PADDING + 8, blockY + 7, 24, 14);
        context.fillStyle = COLORS.blue;
        context.font = `bold 9px ${MONO_FONT}`;
        context.fillText(
          String(index + 1).padStart(2, "0"),
          PADDING + 12,
          blockY + 17,
        );

        context.fillStyle = COLORS.white;
        context.font = `11px ${SANS_FONT}`;
        context.fillText(track.name, PADDING + 40, blockY + 19);
        context.globalAlpha = 0.75;
        context.font = `9px ${MONO_FONT}`;
        context.fillText(track.meta, PADDING + 40, blockY + 33);
        context.globalAlpha = 1;
      }
    }
  }

  drawCrosshair(context, PADDING, laneTop);
  drawCrosshair(context, PADDING + usableWidth, laneTop);
  drawCrosshair(context, PADDING, laneTop + laneArea);
  drawCrosshair(context, PADDING + usableWidth, laneTop + laneArea);

  const playhead = Math.min(Math.max(view.playhead, 0), view.duration);
  const playheadX = PADDING + playhead * pixelsPerSecond;
  context.strokeStyle = COLORS.blue;
  context.lineWidth = 1;
  context.beginPath();
  context.moveTo(playheadX, PADDING);
  context.lineTo(playheadX, laneTop + laneArea);
  context.stroke();

  context.fillStyle = COLORS.blue;
  context.beginPath();
  context.moveTo(playheadX - 4, PADDING);
  context.lineTo(playheadX + 4, PADDING);
  context.lineTo(playheadX, PADDING + 7);
  context.closePath();
  context.fill();
}

function drawCrosshair(
  context: CanvasRenderingContext2D,
  x: number,
  y: number,
): void {
  context.strokeStyle = COLORS.line;
  context.lineWidth = 1;
  context.beginPath();
  context.moveTo(x - 4, y);
  context.lineTo(x + 4, y);
  context.moveTo(x, y - 4);
  context.lineTo(x, y + 4);
  context.stroke();
}

export function secondsAtX(
  canvas: HTMLCanvasElement,
  clientX: number,
  duration: number,
): number {
  const rect = canvas.getBoundingClientRect();
  const usableWidth = rect.width - PADDING * 2;
  if (usableWidth <= 0) return 0;
  const ratio = (clientX - rect.left - PADDING) / usableWidth;
  return Math.min(duration, Math.max(0, ratio * duration));
}

export type LoopEdge = "start" | "end";

export function hitTestLoop(
  canvas: HTMLCanvasElement,
  view: TimelineView,
  clientX: number,
  clientY: number,
): LoopEdge | null {
  const rect = canvas.getBoundingClientRect();
  if (rect.width === 0 || rect.height === 0 || view.duration <= 0) return null;
  const x = clientX - rect.left;
  const y = clientY - rect.top;
  if (y < PADDING || y > PADDING + RULER_HEIGHT + 6) return null;
  const usableWidth = rect.width - PADDING * 2;
  const pixelsPerSecond = usableWidth / view.duration;
  const startX = PADDING + view.loopStart * pixelsPerSecond;
  const endX = PADDING + view.loopEnd * pixelsPerSecond;
  const startDistance = Math.abs(x - startX);
  const endDistance = Math.abs(x - endX);
  if (Math.min(startDistance, endDistance) > 7) return null;
  return startDistance <= endDistance ? "start" : "end";
}

export function trackAtY(
  canvas: HTMLCanvasElement,
  trackCount: number,
  clientY: number,
): number | null {
  if (trackCount <= 0) return null;
  const rect = canvas.getBoundingClientRect();
  if (rect.height === 0) return null;
  const laneTop = PADDING + RULER_HEIGHT + 6;
  const laneArea = rect.height - laneTop - PADDING;
  const laneHeight = laneArea / trackCount;
  if (laneHeight <= 0) return null;
  const raw = Math.floor((clientY - rect.top - laneTop) / laneHeight);
  return Math.min(trackCount - 1, Math.max(0, raw));
}

export function hitTestTimeline(
  canvas: HTMLCanvasElement,
  view: TimelineView,
  clientX: number,
  clientY: number,
): TimelineHit | null {
  const rect = canvas.getBoundingClientRect();
  const width = rect.width;
  const height = rect.height;
  if (width === 0 || height === 0 || view.duration <= 0) return null;

  const x = clientX - rect.left;
  const y = clientY - rect.top;
  const laneTop = PADDING + RULER_HEIGHT + 6;
  const laneArea = height - laneTop - PADDING;
  const usableWidth = width - PADDING * 2;
  const pixelsPerSecond = usableWidth / view.duration;
  const laneHeight = laneArea / Math.max(1, view.tracks.length);

  for (let index = view.tracks.length - 1; index >= 0; index -= 1) {
    const track = view.tracks[index];
    if (!track) continue;

    const laneY = laneTop + index * laneHeight + LANE_GAP / 2;
    const laneH = laneHeight - LANE_GAP;
    const clipTop = laneY + 16;
    const clipHeight = Math.max(18, laneH - 20);
    const noteY = clipTop + clipHeight - 8;

    for (const clip of track.clips) {
      const clipX = PADDING + clip.start * pixelsPerSecond;
      const clipW = Math.max(2, clip.duration * pixelsPerSecond);

      for (const note of clip.notes) {
        const noteX = clipX + note.start * pixelsPerSecond;
        const noteW = Math.max(2, note.duration * pixelsPerSecond);
        if (
          x >= noteX &&
          x <= noteX + noteW &&
          y >= noteY - 6 &&
          y <= noteY + 8
        ) {
          const mode: TimelineDragMode =
            x >= noteX + noteW - 5
              ? "resize-end"
              : x <= noteX + 5
                ? "resize-start"
                : "move";
          return {
            kind: "note",
            trackId: track.id,
            clipId: clip.id,
            noteId: note.id,
            mode,
          };
        }
      }

      if (
        x >= clipX &&
        x <= clipX + clipW &&
        y >= clipTop &&
        y <= clipTop + clipHeight
      ) {
        const mode: TimelineDragMode =
          x >= clipX + clipW - 6
            ? "resize-end"
            : x <= clipX + 6
              ? "resize-start"
              : "move";
        return { kind: "clip", trackId: track.id, clipId: clip.id, mode };
      }
    }
  }

  return null;
}
