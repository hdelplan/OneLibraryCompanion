// Envelope caching adapted from P2GR/DJM-Rec-for-Android.
// Native PWV7 heights are already display envelopes; do not sqrt them again.
// MIT attribution: THIRD_PARTY_NOTICES.md. Position comes from CDJ telemetry.
import type { Wave } from "./model";
const tileSize = 512;
const cache = new WeakMap<Wave, Map<string, Path2D[]>>();
export function bandHeight(value: number): number {
  return Number.isFinite(value) ? Math.max(0, Math.min(1, value)) : 0;
}
export function drawBands(
  ctx: CanvasRenderingContext2D,
  wave: Wave,
  start: number,
  visible: number,
  width: number,
  height: number,
  emphasis: boolean,
) {
  let tiles = cache.get(wave);
  if (!tiles) {
    tiles = new Map();
    cache.set(wave, tiles);
  }
  const first = Math.max(0, Math.floor(start / tileSize));
  const last = Math.min(
    Math.ceil(wave.columns.length / tileSize) - 1,
    Math.floor((start + visible) / tileSize),
  );
  const masks = [1, 2, 4, 3, 5, 6, 7];
  const colors = [
    "#0055e1",
    "#ffa600",
    "#ffffff",
    "#b4690a",
    "#d2dcfa",
    "#fff0d7",
    "#f5ebd7",
  ];
  const entries: { index: number; paths: Path2D[] }[] = [];
  for (let tile = first; tile <= last; tile++) {
    const key = `${tile}:${emphasis}`;
    let paths = tiles.get(key);
    if (!paths) {
      paths = masks.map(() => new Path2D());
      const offset = tile * tileSize;
      const count = Math.min(tileSize, wave.columns.length - offset - 1);
      for (let band = 0; band < masks.length; band++) {
        const amplitude = (i: number) => {
          const c = wave.columns[offset + i];
          const heights = [c.low * (emphasis ? 1.15 : 1), c.mid, c.high];
          return Math.min(
            ...heights.filter((_, i) => masks[band] & (1 << i)).map(bandHeight),
          );
        };
        paths[band].moveTo(0, -amplitude(0));
        for (let i = 1; i <= count; i++) paths[band].lineTo(i, -amplitude(i));
        for (let i = count; i >= 0; i--) paths[band].lineTo(i, amplitude(i));
        paths[band].closePath();
      }
      tiles.set(key, paths);
      // Keep nearby geometry bounded, even over a long track or many seeks.
      if (tiles.size > 32) tiles.delete(tiles.keys().next().value!);
    }
    entries.push({ index: tile, paths });
  }
  ctx.save();
  ctx.beginPath();
  ctx.rect(0, 0, width, height);
  ctx.clip();
  ctx.translate(0, height / 2);
  ctx.scale(width / visible, Math.max(1, height / 2 - 14) * 0.91);
  for (let band = 0; band < masks.length; band++) {
    ctx.fillStyle = colors[band];
    for (const entry of entries) {
      ctx.save();
      ctx.translate(entry.index * tileSize - start, 0);
      ctx.fill(entry.paths[band]);
      ctx.restore();
    }
  }
  ctx.restore();
}
