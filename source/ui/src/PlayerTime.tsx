import { useEffect, useRef, useState } from "react";
import { PlaybackClock, type Motion } from "./playbackClock";
import { playerTimeLabel } from "./settings";
export function PlayerTime({
  position,
  duration,
  remaining,
  milliseconds,
  motion,
}: {
  position: number | null;
  duration: number | null;
  remaining: boolean;
  milliseconds: boolean;
  motion?: Motion;
}) {
  const latest = useRef({
    position,
    duration,
    remaining,
    milliseconds,
    motion,
  });
  latest.current = { position, duration, remaining, milliseconds, motion };
  const [text, setText] = useState(() => playerTimeLabel(null, milliseconds));
  useEffect(() => {
    const clock = new PlaybackClock();
    let frame = 0,
      last = 0;
    const render = (now: number) => {
      if (now - last >= 30) {
        const p = latest.current;
        if (p.motion) clock.update(p.motion);
        const pos =
          p.position === null ? null : p.motion ? clock.read(now) : p.position;
        const value =
          pos === null
            ? null
            : p.remaining
              ? p.duration === null
                ? null
                : p.duration - pos
              : pos;
        setText(playerTimeLabel(value, p.milliseconds));
        last = now;
      }
      frame = requestAnimationFrame(render);
    };
    frame = requestAnimationFrame(render);
    return () => cancelAnimationFrame(frame);
  }, []);
  return (
    <strong className="player-time">
      {milliseconds ? (
        <>
          {text.split(".")[0]}.
          <span className="time-fraction">{text.split(".")[1] ?? "---"}</span>
        </>
      ) : (
        text.split(/([MSF])/).map((part, i) =>
          /^[MSF]$/.test(part) ? (
            <span className="time-unit" key={i}>
              {part}
            </span>
          ) : (
            part
          ),
        )
      )}
    </strong>
  );
}
