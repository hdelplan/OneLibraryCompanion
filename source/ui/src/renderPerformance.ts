// Retain the last visible waveform measurements when MENU replaces the canvases.
const samples = new Map<HTMLCanvasElement, Record<string, unknown>>();
export function recordRenderPerformance(
  canvas: HTMLCanvasElement,
  stats: Record<string, unknown>,
) {
  samples.set(canvas, stats);
  while (samples.size > 2) samples.delete(samples.keys().next().value!);
}
export function renderPerformanceReport() {
  return JSON.stringify(
    { capturedAt: new Date().toISOString(), waveforms: [...samples.values()] },
    null,
    2,
  );
}
