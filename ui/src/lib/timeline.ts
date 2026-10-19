export type Clip = {
  id: string;
  name: string;
  start: number;
  duration: number;
};

export type TimelineView = {
  trackName: string;
  duration: number;
  playhead: number;
  clips: Clip[];
};

const PADDING = 14;
const RULER_HEIGHT = 26;

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

  // 点阵底
  context.fillStyle = COLORS.dot;
  for (let x = 10; x < width; x += 24) {
    for (let y = 10; y < height; y += 24) {
      context.fillRect(x, y, 1, 1);
    }
  }

  const laneTop = PADDING + RULER_HEIGHT + 6;
  const laneHeight = height - laneTop - PADDING;
  const usableWidth = width - PADDING * 2;
  const pixelsPerSecond = usableWidth / view.duration;

  // 标尺基线
  context.strokeStyle = COLORS.line;
  context.lineWidth = 1;
  context.beginPath();
  context.moveTo(PADDING, PADDING + RULER_HEIGHT);
  context.lineTo(PADDING + usableWidth, PADDING + RULER_HEIGHT);
  context.stroke();

  // 刻度
  context.font = "10px Consolas, monospace";
  for (let i = 0; i <= view.duration * 2; i += 1) {
    const second = i / 2;
    const x = PADDING + second * pixelsPerSecond;
    const major = i % 2 === 0;
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

  // 轨道框
  context.strokeStyle = COLORS.line;
  context.strokeRect(
    PADDING + 0.5,
    laneTop + 0.5,
    usableWidth - 1,
    laneHeight - 1,
  );

  // 纵向虚线
  context.setLineDash([2, 4]);
  context.strokeStyle = COLORS.grid;
  for (let second = 0; second <= view.duration; second += 1) {
    const x = PADDING + second * pixelsPerSecond;
    context.beginPath();
    context.moveTo(x, laneTop);
    context.lineTo(x, laneTop + laneHeight);
    context.stroke();
  }
  context.setLineDash([]);

  // 片段
  for (const clip of view.clips) {
    const x = PADDING + clip.start * pixelsPerSecond;
    const clipWidth = Math.max(2, clip.duration * pixelsPerSecond);
    const y = laneTop + 12;
    const clipHeight = Math.min(84, laneHeight - 24);

    context.fillStyle = COLORS.blue;
    context.fillRect(x, y, clipWidth, clipHeight);
    context.strokeStyle = COLORS.blueDark;
    context.strokeRect(x + 0.5, y + 0.5, clipWidth - 1, clipHeight - 1);

    context.fillStyle = COLORS.white;
    context.fillRect(x + 8, y + 8, 22, 14);
    context.fillStyle = COLORS.blue;
    context.font = "bold 9px Consolas, monospace";
    context.fillText("01", x + 13, y + 18);

    context.fillStyle = COLORS.white;
    context.font = "11px 'Bahnschrift', 'Segoe UI', sans-serif";
    context.fillText(clip.name, x + 8, y + 40);

    context.globalAlpha = 0.75;
    context.font = "9px Consolas, monospace";
    context.fillText(`${clip.duration.toFixed(2)} S`, x + 8, y + 55);
    context.globalAlpha = 1;
  }

  // 四角十字标记
  drawCrosshair(context, PADDING, laneTop);
  drawCrosshair(context, PADDING + usableWidth, laneTop);
  drawCrosshair(context, PADDING, laneTop + laneHeight);
  drawCrosshair(context, PADDING + usableWidth, laneTop + laneHeight);

  // 播放头
  const playhead = Math.min(Math.max(view.playhead, 0), view.duration);
  const playheadX = PADDING + playhead * pixelsPerSecond;
  context.strokeStyle = COLORS.blue;
  context.lineWidth = 1;
  context.beginPath();
  context.moveTo(playheadX, PADDING);
  context.lineTo(playheadX, laneTop + laneHeight);
  context.stroke();

  context.fillStyle = COLORS.blue;
  context.beginPath();
  context.moveTo(playheadX - 4, PADDING);
  context.lineTo(playheadX + 4, PADDING);
  context.lineTo(playheadX, PADDING + 7);
  context.closePath();
  context.fill();

  // 底部标注
  context.fillStyle = COLORS.muted;
  context.font = "9px Consolas, monospace";
  context.fillText(view.trackName.toUpperCase(), PADDING, height - 3);
  context.textAlign = "right";
  context.fillText(
    `T+${view.playhead.toFixed(2)} S`,
    PADDING + usableWidth,
    height - 3,
  );
  context.textAlign = "left";
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
