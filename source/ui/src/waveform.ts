import type { Column, Wave } from "./model";
import { drawBands } from "./bandGeometry";
// Native-band scaling follows Beat Link's PWV7 renderer; overlap palette
// follows the CDJ-3000 band-set codec. See docs/research/latency-and-waveforms-2026-09-30.md.
export const rx3Colors = { low: "#0055e1", mid: "#ffa600", high: "#ffffff" };
const bandColors = [
  "#000000",
  "#0055e1",
  "#ffa600",
  "#b4690a",
  "#ffffff",
  "#d2dcfa",
  "#fff0d7",
  "#f5ebd7",
];
export function rx3Layers(
  c: Column,
  emphasis = false,
): { height: number; color: string }[] {
  const values = [
    Math.min(1, Math.max(0, c.low) * (emphasis ? 1.15 : 1)),
    Math.min(1, Math.max(0, c.mid)),
    Math.min(1, Math.max(0, c.high)),
  ];
  return [...new Set(values)]
    .filter((h) => h > 0)
    .sort((a, b) => b - a)
    .map((height) => ({
      height,
      color:
        bandColors[
          values.reduce(
            (mask, value, i) => mask | (value >= height ? 1 << i : 0),
            0,
          )
        ],
    }));
}
// PWV6 stores additive contributions, unlike the overlapping PWV7 detail.
export function overviewLayers(
  c: Column,
  peakHeight: number,
): { height: number; color: string }[] {
  if (peakHeight <= 0) return [];
  const low = c.low * 0.49,
    mid = c.mid * 0.32,
    high = c.high * 0.25;
  return [
    { height: (low + mid + high) / peakHeight, color: rx3Colors.high },
    { height: (low + mid) / peakHeight, color: rx3Colors.mid },
    { height: low / peakHeight, color: rx3Colors.low },
  ];
}
const overviewPeaks = new WeakMap<Wave, number>();
function overviewPeak(wave: Wave): number {
  let result = overviewPeaks.get(wave);
  if (result === undefined) {
    result = wave.columns.reduce(
      (m, c) => Math.max(m, c.low * 0.49 + c.mid * 0.32 + c.high * 0.25),
      0,
    );
    overviewPeaks.set(wave, result);
  }
  return result;
}

// Max pooling preserves transients when several time samples land in one pixel.
export function peak(columns: Column[], start: number, end: number): Column {
  const p = { low: 0, mid: 0, high: 0 };
  for (
    let i = Math.max(0, Math.floor(start));
    i <
    Math.min(columns.length, Math.max(Math.floor(start) + 1, Math.ceil(end)));
    i++
  ) {
    p.low = Math.max(p.low, columns[i].low);
    p.mid = Math.max(p.mid, columns[i].mid);
    p.high = Math.max(p.high, columns[i].high);
  }
  return p;
}
export function viewportStart(
  position: number,
  rate: number,
  span: number,
  anchor: number,
): number {
  return position * rate - span * rate * anchor;
}
// Zoom is measured in playback seconds, while analysis uses source-track seconds.
// Keep the same scale when paused or reversed; invalid rates use normal speed.
export function sourceSpan(span: number, playbackRate = 1): number {
  return (
    span *
    (Number.isFinite(playbackRate) && playbackRate !== 0
      ? Math.abs(playbackRate)
      : 1)
  );
}
export type DrawOptions = {
  playbackRate?: number;
  beats?: { time: number; beatInBar: number }[];
  cues?: { time: number; hot: number; label: string; color: string }[];
  loop?: { start: number; end: number; estimated: boolean } | null;
  anchor?: number;
  playheadColor?: string;
  emphasis?: boolean;
  duration?: number;
};
// The full-track overview is stationary. Rasterize its bands once per size;
// only transport/cue/loop overlays need repainting as the player moves.
const overviewImages = new WeakMap<
  HTMLCanvasElement,
  {
    wave: Wave;
    width: number;
    height: number;
    image: HTMLCanvasElement;
  }
>();
export function draw(
  canvas: HTMLCanvasElement,
  wave: Wave | null,
  position: number,
  overview = false,
  span = 8,
  options: DrawOptions = {},
) {
  const width = canvas.clientWidth,
    height = canvas.clientHeight,
    dpr = window.devicePixelRatio || 1;
  if (canvas.width !== Math.round(width * dpr))
    canvas.width = Math.round(width * dpr);
  if (canvas.height !== Math.round(height * dpr))
    canvas.height = Math.round(height * dpr);
  const ctx = canvas.getContext("2d");
  if (!ctx || width <= 0 || height <= 0) return;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.fillStyle = "#000";
  ctx.fillRect(0, 0, width, height);
  if (!wave || !wave.columns.length) return;
  if (!overview) span = sourceSpan(span, options.playbackRate);
  const emphasis = options.emphasis ?? true,
    anchor = options.anchor ?? 1 / 3;
  const columns = wave.columns;
  const previewPeak = overview ? overviewPeak(wave) : 0;
  const rate = wave.samplesPerSecond ?? 150;
  const visible = overview ? columns.length : span * rate;
  const start = overview ? 0 : viewportStart(position, rate, span, anchor);
  // Render one bar per physical pixel so quiet detail survives high-density displays.
  const pixels = Math.round(width * dpr);
  if (!overview) drawBands(ctx, wave, start, visible, width, height, emphasis);
  if (overview) {
    let cached = overviewImages.get(canvas);
    if (
      !cached ||
      cached.wave !== wave ||
      cached.width !== canvas.width ||
      cached.height !== canvas.height
    ) {
      const image = document.createElement("canvas");
      image.width = canvas.width;
      image.height = canvas.height;
      const paint = image.getContext("2d")!;
      paint.scale(dpr, dpr);
      for (let x = 0; x < pixels; x++) {
        const at = (x / pixels) * visible;
        const c = peak(columns, at, ((x + 1) / pixels) * visible);
        for (const band of overviewLayers(c, previewPeak)) {
          const h = band.height * (height - 2);
          paint.fillStyle = band.color;
          paint.fillRect(x / dpr, height - h, 1 / dpr, h);
        }
      }
      cached = { wave, width: canvas.width, height: canvas.height, image };
      overviewImages.set(canvas, cached);
    }
    ctx.drawImage(cached.image, 0, 0, width, height);
  }
  if (!overview && options.beats) {
    const startTime = position - span * anchor;
    // Binary search avoids scanning a full track on every animation frame.
    let lo = 0,
      hi = options.beats.length;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if (options.beats[mid].time < startTime) lo = mid + 1;
      else hi = mid;
    }
    for (let i = lo; i < options.beats.length; i++) {
      const beat = options.beats[i];
      const x = ((beat.time - startTime) / span) * width;
      if (x > width) break;
      ctx.fillStyle = beat.beatInBar === 1 ? "#ff4040" : "#ffffff";
      ctx.fillRect(x, 1, 1.5, 7);
      ctx.fillRect(x, height - 8, 1.5, 7);
    }
  }
  const duration = options.duration ?? 0;
  if (options.loop && options.loop.end > options.loop.start) {
    const viewStart = overview ? 0 : position - span * anchor;
    const viewSpan = overview ? duration : span;
    if (viewSpan > 0) {
      const left = Math.max(
        0,
        Math.min(width, ((options.loop.start - viewStart) / viewSpan) * width),
      );
      const right = Math.max(
        0,
        Math.min(width, ((options.loop.end - viewStart) / viewSpan) * width),
      );
      if (right > left) {
        ctx.fillStyle = "rgba(255, 191, 0, 0.20)";
        ctx.fillRect(left, 0, right - left, height);
      }
    }
  }
  if (!overview || duration > 0) {
    for (const cue of options.cues ?? []) {
      const x = overview
        ? (cue.time / duration) * width
        : ((cue.time - position + span * anchor) / span) * width;
      if (x < 0 || x > width) continue;
      ctx.fillStyle = cue.color;
      ctx.fillRect(x, 15, 1, height - 15);
      if (cue.hot) {
        ctx.fillRect(x - 7, 0, 14, 14);
        ctx.fillStyle = "#000";
        ctx.font = "bold 10px sans-serif";
        ctx.textAlign = "center";
        ctx.fillText(cue.label, x, 11);
      } else {
        ctx.beginPath();
        ctx.moveTo(x - 5, 1);
        ctx.lineTo(x + 5, 1);
        ctx.lineTo(x, 9);
        ctx.closePath();
        ctx.fill();
      }
    }

    const fraction = overview
      ? Math.max(0, Math.min(1, position / duration))
      : anchor;
    const x = Math.min(
      width - 2,
      Math.max(0, Math.round(width * fraction) - 1),
    );
    ctx.fillStyle = options.playheadColor ?? "#eeeeee";
    ctx.fillRect(x, 0, 2, height);
    if (!overview) {
      ctx.beginPath();
      ctx.moveTo(x - 4, 0);
      ctx.lineTo(x + 6, 0);
      ctx.lineTo(x + 1, 6);
      ctx.fill();
    }
  }
}
