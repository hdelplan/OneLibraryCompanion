import { recordJogFrame } from "./jogTrace";
import { useEffect, useRef } from "react";
import type { Wave, LivePlayer, Analysis } from "./model";
import type { Settings } from "./settings";
import { draw } from "./waveform";
import { PlaybackClock, type Motion } from "./playbackClock";
export function Signal({
  wave,
  position,
  settings,
  overview = false,
  duration = 0,
  motion,
  loop,
  cues,
  beats,
  onZoom,
}: {
  wave: Wave | null;
  position: number;
  settings: Settings;
  overview?: boolean;
  duration?: number;
  motion?: Motion;
  loop?: LivePlayer["loop"];
  cues?: Analysis["cues"];
  beats?: Analysis["beats"];
  onZoom?: (window: number) => void;
}) {
  const ref = useRef<HTMLCanvasElement>(null);
  const latest = useRef({
    wave,
    position,
    settings,
    overview,
    duration,
    motion,
    loop,
    cues,
    beats,
  });
  latest.current = {
    wave,
    position,
    settings,
    overview,
    duration,
    motion,
    loop,
    cues,
    beats,
  };
  useEffect(() => {
    const canvas = ref.current!;
    const clock = new PlaybackClock();
    let frame = 0,
      dirty = true,
      lastPosition = -1,
      lastProps = latest.current;
    let longFrames = 0,
      worstFrame = 0,
      positionJumps = 0,
      previousPosition: number | null = null;
    let previousFrame = 0,
      measuredAt = 0;
    const intervals: number[] = [],
      costs: number[] = [];
    const render = (now: number) => {
      const frameDelta = previousFrame ? now - previousFrame : 0;
      if (previousFrame && document.visibilityState === "visible") {
        if (frameDelta > 25) longFrames++;
        worstFrame = Math.max(worstFrame, frameDelta);
        intervals.push(now - previousFrame);
        if (intervals.length > 120) intervals.shift();
      }
      previousFrame = now;
      const p = latest.current;
      if (p.motion)
        clock.update({ ...p.motion, smoothMs: p.settings.jogSmoothing });
      const position = p.motion ? clock.read(now) : p.position;

      if (
        previousPosition !== null &&
        p.motion?.playing &&
        !p.motion.direct &&
        !p.motion.loop &&
        frameDelta > 0 &&
        Math.abs(
          position - previousPosition - (frameDelta / 1000) * p.motion.rate,
        ) > 0.05
      )
        positionJumps++;
      previousPosition = position;
      let drawCost = 0;
      if (dirty || p !== lastProps || position !== lastPosition) {
        const drawAt = performance.now();
        draw(canvas, p.wave, position, p.overview, p.settings.window, {
          anchor: p.settings.playhead === "third" ? 1 / 3 : 0.5,
          playheadColor:
            p.settings.playheadColor === "red" ? "#ff0000" : "#eeeeee",
          emphasis: p.settings.bass === "emphasis",
          duration: p.duration,
          playbackRate: p.motion?.tempoRate ?? p.motion?.rate,
          loop: p.loop,
          cues: p.cues,
          beats: p.beats,
        });
        drawCost = performance.now() - drawAt;
        costs.push(drawCost);
        if (costs.length > 120) costs.shift();
        lastProps = p;
        lastPosition = position;
        dirty = false;
      }
      if (!p.overview)
        recordJogFrame(
          p.motion,
          position,
          now,
          frameDelta,
          p.settings.jogSmoothing,
          drawCost,
        );
      if (now - measuredAt > 1000 && intervals.length) {
        const sorted = [...intervals].sort((a, b) => a - b);
        canvas.dataset.renderStats = JSON.stringify({
          longFrames,
          worstFrame,
          positionJumps,
          frameMedianMs: sorted[Math.floor(sorted.length / 2)],
          frameP95Ms: sorted[Math.floor(sorted.length * 0.95)],
          drawMeanMs:
            costs.reduce((a, b) => a + b, 0) / Math.max(1, costs.length),
          drawMaxMs: Math.max(0, ...costs),
          playing: p.motion?.playing,
          direct: p.motion?.direct,
        });
        measuredAt = now;
      }
      frame = requestAnimationFrame(render);
    };
    const observer = new ResizeObserver(() => {
      dirty = true;
    });
    observer.observe(canvas);
    frame = requestAnimationFrame(render);
    return () => {
      observer.disconnect();
      cancelAnimationFrame(frame);
    };
  }, []);
  const pinch = useRef<{ distance: number; window: number } | null>(null);
  return (
    <canvas
      style={{ touchAction: onZoom ? "none" : "auto" }}
      onTouchStart={(e) => {
        if (!onZoom || e.touches.length !== 2) return;
        pinch.current = {
          distance: Math.hypot(
            e.touches[0].clientX - e.touches[1].clientX,
            e.touches[0].clientY - e.touches[1].clientY,
          ),
          window: settings.window,
        };
      }}
      onTouchMove={(e) => {
        if (!onZoom || !pinch.current || e.touches.length !== 2) return;
        const distance = Math.hypot(
          e.touches[0].clientX - e.touches[1].clientX,
          e.touches[0].clientY - e.touches[1].clientY,
        );
        if (distance > 0)
          onZoom(
            Math.max(
              2,
              Math.min(
                32,
                Math.round(
                  (pinch.current.window * pinch.current.distance) /
                    distance /
                    2,
                ) * 2,
              ),
            ),
          );
      }}
      onTouchEnd={() => {
        pinch.current = null;
      }}
      onTouchCancel={() => {
        pinch.current = null;
      }}
      ref={ref}
      aria-label={
        overview ? "Track overview waveform" : "Scrolling three-band waveform"
      }
    />
  );
}
