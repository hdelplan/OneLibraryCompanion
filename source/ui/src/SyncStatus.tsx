import { useEffect, useRef, useState } from "react";
import type { Deck } from "./model";
import { PlaybackClock } from "./playbackClock";
import { beatPhase, phaseDistance, PhaseMismatch } from "./syncPhase";
export function SyncStatus({
  decks,
  index,
  smoothMs,
}: {
  decks: [Deck, Deck];
  index: number;
  smoothMs: number;
}) {
  const latest = useRef(decks);
  latest.current = decks;
  const [flashing, setFlashing] = useState(false);
  useEffect(() => {
    const clocks = [new PlaybackClock(), new PlaybackClock()];
    let mismatch = new PhaseMismatch(),
      identity = "";
    const tick = () => {
      const current = latest.current;
      const nextIdentity = current
        .map((d) => `${d?.live?.number}:${d?.live?.trackKey}`)
        .join("|");
      if (identity !== nextIdentity) {
        mismatch = new PhaseMismatch();
        identity = nextIdentity;
      }
      const now = performance.now();
      const phases = current.map((deck, i) => {
        const motion = deck?.motion;
        if (
          !motion ||
          !motion.playing ||
          motion.direct ||
          deck?.live?.connection !== "connected" ||
          now - (motion.observedAt ?? motion.receivedAt) > 1000 ||
          !["beat", "fine"].includes(motion.quality ?? "")
        )
          return null;
        clocks[i].update({ ...motion, smoothMs });
        return beatPhase(clocks[i].read(now), deck.analysis.beats);
      });
      const error =
        current[index]?.live?.sync === true &&
        phases[0] !== null &&
        phases[1] !== null
          ? phaseDistance(phases[0], phases[1])
          : null;
      setFlashing(mismatch.update(error, now));
    };
    tick();
    const timer = setInterval(tick, 50);
    return () => clearInterval(timer);
  }, [index, smoothMs]);
  const sync = decks[index]?.live?.sync;
  return (
    <span
      className={`sync-status${flashing ? " sync-flashing" : ""}`}
      title={
        flashing
          ? "SYNC on · beat markers out of phase"
          : "Reported CDJ SYNC status"
      }
    >
      SYNC {sync == null ? "—" : sync ? "ON" : "OFF"}
    </span>
  );
}
