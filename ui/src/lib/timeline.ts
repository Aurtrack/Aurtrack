export type TimelineTrack = {
  id: string;
  name: string;
  frequency: number | null;
  meta: string;
};

export type TimelineView = {
  tracks: TimelineTrack[];
  duration: number;
  playhead: number;
  empty: string;
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

  context.font = `10px ${MONO_FONT}`;
  for (let index = 0; index <= view.duration * 2; index += 1) {
    const second = index / 2;
    const x = PADDING + second * pixelsPerSecond;
    const major = index % 2 === 0;
    context.strokeStyle = major ? COLORS.line : COLORS.grid;
    context.beginPath();
    context.moveTo(x, PADDING + (major ? 14 : 20));
    context.lineTo(x, PADDING + RULER_HEIGHT);
    context.stroke();
    if (major) {
      context.fillStyle = COLORS.muted;
      context.fillText(`${second}s`, x + 4, PADDING + 10);
    }
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

      const blockY = laneY + 5;
      const blockH = Math.max(22, laneH - 10);

      if (track.frequency === null) {
        context.strokeStyle = COLORS.line;
        context.strokeRect(
          PADDING + 0.5,
          blockY + 0.5,
          usableWidth - 1,
          blockH - 1,
        );
        context.fillStyle = COLORS.muted;
        context.font = `11px ${SANS_FONT}`;
        context.fillText(track.name, PADDING + 10, blockY + 18);
        context.font = `9px ${MONO_FONT}`;
        context.fillText(track.meta, PADDING + 10, blockY + 32);
      } else {
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
