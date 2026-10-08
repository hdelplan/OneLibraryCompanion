import { playedTrackKey } from "./setHistoryModel";
import type { Deck, LivePlayer } from "./model";
export function loadBlocked(player: LivePlayer | undefined) {
  return (
    !!player &&
    (player.loadProtected === true ||
      player.playing === true ||
      [
        "playing",
        "looping",
        "cue play",
        "cue scratch",
        "searching",
        "emergency loop",
      ].includes(player.playState ?? ""))
  );
}
export function sameLoadPlayers(a: LivePlayer[], b: LivePlayer[]) {
  return (
    a.length === b.length &&
    a.every((p, i) => {
      const q = b[i];
      return (
        p.number === q.number &&
        p.name === q.name &&
        p.sourceLabel === q.sourceLabel &&
        p.connection === q.connection &&
        p.loadBlockedReason === q.loadBlockedReason &&
        loadBlocked(p) === loadBlocked(q)
      );
    })
  );
}

export function currentlyPlayingTrackKeys(decks: Deck[]): string[] {
  return [
    ...new Set(
      decks.flatMap((deck) => {
        const player = deck?.live;
        const track = deck?.analysis.track;
        if (
          !player ||
          player.connection !== "connected" ||
          !track ||
          !(
            player.playing ??
            ["playing", "looping"].includes(player.playState ?? "")
          )
        )
          return [];
        const key = playedTrackKey(track);
        return key ? [key] : [];
      }),
    ),
  ].sort();
}
