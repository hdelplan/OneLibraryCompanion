import type { LivePlayer } from "./model";
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
        loadBlocked(p) === loadBlocked(q)
      );
    })
  );
}
