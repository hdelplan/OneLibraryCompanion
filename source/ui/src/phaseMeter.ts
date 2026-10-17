import type { Analysis, Deck } from "./model";

export function beatAt(position: number, beats: Analysis["beats"]) {
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
  if (period <= 0) return null;
  return {
    index: low,
    fraction: (position - beats[low].time) / period,
    period,
  };
}

export function comparisonReady(deck: Deck, now: number): boolean {
  if (!deck) return false;
  if (deck.source === "offline") return deck.position !== null;
  const motion = deck.motion;
  return (
    !!motion &&
    motion.playing &&
    !motion.direct &&
    deck.live?.connection === "connected" &&
    now - (motion.observedAt ?? motion.receivedAt) <= 1000 &&
    (motion.quality === "beat" || motion.quality === "fine")
  );
}
