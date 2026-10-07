import type { Analysis } from "./model";

// Fractional grid indices preserve exported tempo changes and off-grid hot cues.
function beatIndex(
  beats: NonNullable<Analysis["beats"]>,
  time: number,
): number | null {
  if (
    beats.length < 2 ||
    time < beats[0].time ||
    time > beats[beats.length - 1].time
  )
    return null;
  let lo = 0,
    hi = beats.length - 1;
  while (lo + 1 < hi) {
    const mid = (lo + hi) >>> 1;
    if (beats[mid].time <= time) lo = mid;
    else hi = mid;
  }
  const span = beats[hi].time - beats[lo].time;
  return span > 0 ? lo + (time - beats[lo].time) / span : null;
}
export function cueCountdown(
  position: number | null,
  beats: Analysis["beats"],
  cues: Analysis["cues"],
): { text: string; label: string } {
  const empty = { text: "—.—", label: "NEXT HOT CUE" };
  if (position === null || !Number.isFinite(position) || !beats) return empty;
  const cue = cues
    ?.filter((c) => c.hot > 0 && c.time >= position)
    .sort((a, b) => a.time - b.time)[0];
  if (!cue) return empty;
  const from = beatIndex(beats, position),
    to = beatIndex(beats, cue.time);
  if (from === null || to === null) return empty;
  const count = Math.max(0, Math.ceil(to - from - 1e-7));
  return {
    text: `${String(Math.floor(count / 4)).padStart(2, "0")}.${count % 4}`,
    label: `HOT CUE ${cue.label}`,
  };
}
