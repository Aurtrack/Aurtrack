import type { Clip } from "./api";

export const PIANO_LOW = 48;
export const PIANO_HIGH = 84;
export const PIANO_ROW = 8;
export const PIANO_LEFT = 44;
export const PIANO_PADDING = 12;
export const PIANO_HEADER = 18;
export const PIANO_HEIGHT =
  PIANO_HEADER + PIANO_PADDING * 2 + (PIANO_HIGH - PIANO_LOW + 1) * PIANO_ROW;

const COLORS = {
  paper: "#ecece8",
  gutter: "#e3e5e6",
  whiteRow: "#f4f4f0",
  blackRow: "#e5e7e8",
  line: "#c8cdd3",
  strong: "#98a3b2",
  blue: "#2f5fd0",
  blueDark: "#1e3f8f",
  ink: "#1b1f26",
  muted: "#5f6878",
  white: "#ffffff",
};

const MONO_FONT =
  "'Cascadia Mono', Consolas, 'Microsoft YaHei', 'PingFang SC', monospace";

const BLACK_KEYS = new Set([1, 3, 6, 8, 10]);
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

export type PianoDragMode = "move" | "resize-start" | "resize-end";

export type PianoHit =
  | { kind: "note"; noteId: string; mode: PianoDragMode }
  | { kind: "empty"; pitch: number; time: number };

function noteName(pitch: number): string {
  const name = NOTE_NAMES[pitch % 12] ?? "?";
  return `${name}${Math.floor(pitch / 12) - 1}`;
}

function pitchRow(pitch: number): number {
  return PIANO_HIGH - pitch;
}

function rowY(pitch: number): number {
  return PIANO_HEADER + PIANO_PADDING + pitchRow(pitch) * PIANO_ROW;
}

export function drawPiano(
  canvas: HTMLCanvasElement,
  clip: Clip | null,
  selectedNoteId: string,
): void {
  const width = canvas.clientWidth;
  const height = canvas.clientHeight;
  if (width === 0 || height === 0) return;

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

  if (!clip) return;

  const usableWidth = width - PIANO_LEFT - PIANO_PADDING;
  if (usableWidth <= 0) return;
  const pixelsPerSecond = usableWidth / Math.max(0.001, clip.duration);

  context.fillStyle = COLORS.gutter;
  context.fillRect(0, 0, PIANO_LEFT, height);

  for (let pitch = PIANO_HIGH; pitch >= PIANO_LOW; pitch -= 1) {
    const y = rowY(pitch);
    context.fillStyle = BLACK_KEYS.has(pitch % 12)
      ? COLORS.blackRow
      : COLORS.whiteRow;
    context.fillRect(PIANO_LEFT, y, usableWidth, PIANO_ROW);
    context.strokeStyle = COLORS.line;
    context.beginPath();
    context.moveTo(PIANO_LEFT, y + PIANO_ROW);
    context.lineTo(width - PIANO_PADDING, y + PIANO_ROW);
    context.stroke();

    if (pitch % 12 === 0) {
      context.fillStyle = COLORS.muted;
      context.font = `8px ${MONO_FONT}`;
      context.fillText(noteName(pitch), 6, y + PIANO_ROW - 1);
    }
  }

  context.strokeStyle = COLORS.strong;
  context.beginPath();
  context.moveTo(PIANO_LEFT, PIANO_HEADER);
  context.lineTo(PIANO_LEFT, height - PIANO_PADDING);
  context.stroke();

  const steps = Math.ceil(clip.duration / 0.25);
  for (let index = 0; index <= steps; index += 1) {
    const seconds = index * 0.25;
    const x = PIANO_LEFT + seconds * pixelsPerSecond;
    const major = index % 4 === 0;
    context.strokeStyle = major ? COLORS.strong : COLORS.line;
    context.beginPath();
    context.moveTo(x, PIANO_HEADER);
    context.lineTo(x, height - PIANO_PADDING);
    context.stroke();
    if (major) {
      context.fillStyle = COLORS.muted;
      context.font = `8px ${MONO_FONT}`;
      context.fillText(`${seconds}s`, x + 2, PIANO_HEADER - 5);
    }
  }

  context.font = `8px ${MONO_FONT}`;
  for (const note of clip.notes) {
    if (note.pitch < PIANO_LOW || note.pitch > PIANO_HIGH) continue;
    const x = PIANO_LEFT + note.start * pixelsPerSecond;
    const widthPx = Math.max(3, note.duration * pixelsPerSecond);
    const y = rowY(note.pitch) + 0.5;
    context.fillStyle = COLORS.blue;
    context.fillRect(x, y, widthPx, PIANO_ROW - 1);
    context.strokeStyle = COLORS.blueDark;
    context.strokeRect(x + 0.5, y + 0.5, widthPx - 1, PIANO_ROW - 2);

    if (note.id === selectedNoteId) {
      context.strokeStyle = COLORS.ink;
      context.lineWidth = 1.5;
      context.strokeRect(x + 0.5, y + 0.5, widthPx - 1, PIANO_ROW - 2);
      context.lineWidth = 1;
    }

    if (widthPx > 26) {
      context.fillStyle = COLORS.white;
      context.fillText(noteName(note.pitch), x + 3, y + PIANO_ROW - 2);
    }
  }
}

export function hitTestPiano(
  canvas: HTMLCanvasElement,
  clip: Clip | null,
  clientX: number,
  clientY: number,
): PianoHit | null {
  if (!clip) return null;
  const rect = canvas.getBoundingClientRect();
  const width = rect.width;
  const height = rect.height;
  if (width === 0 || height === 0) return null;

  const x = clientX - rect.left;
  const y = clientY - rect.top;
  if (x < PIANO_LEFT || x > width - PIANO_PADDING) return null;
  if (y < PIANO_HEADER + PIANO_PADDING || y > height - PIANO_PADDING) {
    return null;
  }

  const usableWidth = width - PIANO_LEFT - PIANO_PADDING;
  const pixelsPerSecond = usableWidth / Math.max(0.001, clip.duration);
  const row = Math.floor((y - PIANO_HEADER - PIANO_PADDING) / PIANO_ROW);
  const pitch = PIANO_HIGH - row;
  if (pitch < PIANO_LOW || pitch > PIANO_HIGH) return null;

  for (let index = clip.notes.length - 1; index >= 0; index -= 1) {
    const note = clip.notes[index];
    if (!note || note.pitch !== pitch) continue;
    const noteX = PIANO_LEFT + note.start * pixelsPerSecond;
    const noteW = Math.max(3, note.duration * pixelsPerSecond);
    if (x >= noteX && x <= noteX + noteW) {
      const mode: PianoDragMode =
        x >= noteX + noteW - 5
          ? "resize-end"
          : x <= noteX + 5
            ? "resize-start"
            : "move";
      return { kind: "note", noteId: note.id, mode };
    }
  }

  return {
    kind: "empty",
    pitch,
    time: Math.min(
      clip.duration,
      Math.max(0, (x - PIANO_LEFT) / pixelsPerSecond),
    ),
  };
}
