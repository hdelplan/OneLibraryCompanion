import type { Deck } from "./model";

type Key = { number: number; mode: "A" | "B" };
export function camelotKey(value: string): Key | null {
  const normalized = value.trim().replaceAll("♯", "#").replaceAll("♭", "b");
  const compact = normalized
    .toLowerCase()
    .replaceAll("sharp", "#")
    .replaceAll("flat", "b")
    .replace(/\s/g, "");
  const wheel = /^(\d{1,2})([abmd])$/.exec(compact);
  if (wheel) {
    const n = Number(wheel[1]);
    if (n < 1 || n > 12) return null;
    return {
      number: "md".includes(wheel[2]) ? ((n + 6) % 12) + 1 : n,
      mode: "am".includes(wheel[2]) ? "A" : "B",
    };
  }
  const note = /^([a-g](?:#|b)?)(minor|min|major|maj|m)?$/.exec(compact);
  if (!note) return null;
  const roots: Record<string, number> = {
    c: 0,
    "b#": 0,
    "c#": 1,
    db: 1,
    d: 2,
    "d#": 3,
    eb: 3,
    e: 4,
    fb: 4,
    f: 5,
    "e#": 5,
    "f#": 6,
    gb: 6,
    g: 7,
    "g#": 8,
    ab: 8,
    a: 9,
    "a#": 10,
    bb: 10,
    b: 11,
    cb: 11,
  };
  const minor =
    ["minor", "min", "m"].includes(note[2]) && !normalized.endsWith("M");
  const numbers = minor
    ? [5, 12, 7, 2, 9, 4, 11, 6, 1, 8, 3, 10]
    : [8, 3, 10, 5, 12, 7, 2, 9, 4, 11, 6, 1];
  return { number: numbers[roots[note[1]]], mode: minor ? "A" : "B" };
}

// Count playing decks before checking metadata: a second playing deck with an
// unknown key must still disable highlighting. Offline previews never qualify.
export function activeMixKey(decks: Deck[]): string | null {
  const playing = decks.filter(
    (deck) =>
      deck?.live?.connection === "connected" &&
      (deck.live.playing ??
        ["playing", "looping", "cue play", "emergency loop"].includes(
          deck.live.playState ?? "",
        )),
  );
  return playing.length === 1 ? playing[0]?.analysis.track?.key || null : null;
}

export function keyCompatibility(
  reference: string | null,
  candidate: string,
): "compatible" | "semi" | null {
  const from = reference ? camelotKey(reference) : null;
  const to = camelotKey(candidate);
  if (!from || !to) return null;
  const delta = (to.number - from.number + 12) % 12;
  if (delta === 0 || (from.mode === to.mode && [1, 11].includes(delta)))
    return "compatible";
  if (
    (from.mode !== to.mode && [1, 2, 7, 11].includes(delta)) ||
    (from.mode === to.mode && [2, 7].includes(delta))
  )
    return "semi";
  return null;
}
