export function beatDuration(
  barSeconds: number,
  beatsPerBar: number,
): number {
  return Math.max(0.001, barSeconds) / Math.max(1, beatsPerBar);
}

export function beatsToSeconds(
  beats: number,
  barSeconds: number,
  beatsPerBar: number,
): number {
  return beats * beatDuration(barSeconds, beatsPerBar);
}

export function secondsToBeats(
  seconds: number,
  barSeconds: number,
  beatsPerBar: number,
): number {
  return seconds / beatDuration(barSeconds, beatsPerBar);
}

export function formatBarBeat(
  seconds: number,
  barSeconds: number,
  beatsPerBar: number,
): string {
  const bar = Math.floor(Math.max(0, seconds) / Math.max(0.001, barSeconds)) + 1;
  const beat =
    Math.floor(
      (Math.max(0, seconds) % Math.max(0.001, barSeconds)) /
        beatDuration(barSeconds, beatsPerBar),
    ) + 1;
  return `${bar}.${beat}`;
}
