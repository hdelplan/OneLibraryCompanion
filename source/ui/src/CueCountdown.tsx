import { useEffect, useRef, useState } from "react";
import type { Deck } from "./model";
import { PlaybackClock } from "./playbackClock";
import { cueCountdown } from "./cueCountdownModel";
export function CueCountdown({
  deck,
  smoothMs,
}: {
  deck: Deck;
  smoothMs: number;
}) {
  const latest = useRef({ deck, smoothMs });
  latest.current = { deck, smoothMs };
  const [value, setValue] = useState({ text: "—.—", label: "NEXT HOT CUE" });
  useEffect(() => {
    const clock = new PlaybackClock();
    let frame = 0;
    const render = (now: number) => {
      const { deck: d, smoothMs } = latest.current;
      if (d?.motion) clock.update({ ...d.motion, smoothMs });
      const position =
        d?.position == null ? null : d.motion ? clock.read(now) : d.position;
      const next = cueCountdown(position, d?.analysis.beats, d?.analysis.cues);
      setValue((old) =>
        old.text === next.text && old.label === next.label ? old : next,
      );
      frame = requestAnimationFrame(render);
    };
    frame = requestAnimationFrame(render);
    return () => cancelAnimationFrame(frame);
  }, []);
  return (
    <div
      className="cue-countdown"
      aria-label={`Bars and beats to next hot cue: ${value.text}`}
    >
      <small>{value.label}</small>
      <strong>
        {value.text} <span>Bars</span>
      </strong>
    </div>
  );
}
