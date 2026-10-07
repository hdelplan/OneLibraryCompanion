import type { Analysis } from "./model";
export function beatPhase(
  position: number,
  beats: Analysis["beats"],
): number | null {
  if (!beats || beats.length < 2 || !Number.isFinite(position)) return null;
  let low = 0,
    high = beats.length - 1;
  if (position < beats[0].time || position >= beats[high].time) return null;
  while (high - low > 1) {
    const mid = (low + high) >>> 1;
    if (beats[mid].time <= position) low = mid;
    else high = mid;
  }
  const period = beats[high].time - beats[low].time;
  return period > 0 ? (position - beats[low].time) / period : null;
}
export function phaseDistance(a: number, b: number): number {
  const difference = Math.abs(a - b) % 1;
  return Math.min(difference, 1 - difference);
}
export class PhaseMismatch {
  flashing = false;
  private since: number | null = null;
  update(error: number | null, now: number): boolean {
    if (error === null) {
      this.flashing = false;
      this.since = null;
      return false;
    }
    const change = this.flashing ? error < 0.03 : error > 0.05;
    if (!change) this.since = null;
    else if (this.since === null) this.since = now;
    else if (now - this.since >= 250) {
      this.flashing = !this.flashing;
      this.since = null;
    }
    return this.flashing;
  }
}
