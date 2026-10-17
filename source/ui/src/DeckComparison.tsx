import { useEffect, useRef } from "react";
import type { Deck } from "./model";
import type { Settings } from "./settings";
import { PlaybackClock } from "./playbackClock";
import { beatAt, comparisonReady } from "./phaseMeter";
import { draw } from "./waveform";

export function DeckComparison({
  decks,
  index,
  settings,
  onToggle,
}: {
  decks: [Deck, Deck];
  index: number;
  settings: Settings;
  onToggle: () => void;
}) {
  const ref = useRef<HTMLCanvasElement>(null);
  const latest = useRef({ decks, index, settings });
  latest.current = { decks, index, settings };
  useEffect(() => {
    const canvas = ref.current!;
    const clocks = [new PlaybackClock(), new PlaybackClock()];
    let frame = 0;
    const render = (now: number) => {
      const { decks, index, settings } = latest.current;
      const other = 1 - index;
      const positions = decks.map((deck, i) => {
        if (deck?.motion) {
          clocks[i].update({ ...deck.motion, smoothMs: settings.jogSmoothing });
          return clocks[i].read(now);
        }
        return deck?.position ?? 0;
      });
      const rect = canvas.getBoundingClientRect();
      const width = canvas.clientWidth,
        height = canvas.clientHeight;
      const density =
        (devicePixelRatio || 1) * (width ? rect.width / width : 1);
      const pixelWidth = Math.max(1, Math.round(width * density));
      const pixelHeight = Math.max(1, Math.round(height * density));
      if (canvas.width !== pixelWidth) canvas.width = pixelWidth;
      if (canvas.height !== pixelHeight) canvas.height = pixelHeight;
      const ctx = canvas.getContext("2d");
      if (ctx && width && height) {
        ctx.setTransform(density, 0, 0, density, 0, 0);
        ctx.fillStyle = "#252525";
        ctx.fillRect(0, 0, width, height);
        const ready = decks.every((deck) => comparisonReady(deck, now));
        const beats = decks.map((deck, i) =>
          beatAt(positions[i], deck?.analysis.beats),
        );
        if (
          !ready ||
          (settings.phaseDisplay === "phase"
            ? beats.some((b) => !b)
            : !decks[other]?.analysis.detail)
        ) {
          ctx.fillStyle = "#aaaaaa";
          ctx.font = "13px Arial";
          ctx.fillText(
            settings.phaseDisplay === "phase"
              ? "Phase unavailable"
              : "Reference waveform unavailable",
            12,
            height / 2 + 4,
          );
        } else if (settings.phaseDisplay === "waveform") {
          const peer = decks[other]!;
          draw(
            canvas,
            peer.analysis.detail,
            positions[other],
            false,
            settings.window,
            {
              anchor: settings.playhead === "third" ? 1 / 3 : 0.5,
              playheadColor:
                settings.playheadColor === "red" ? "#ff0000" : "#eeeeee",
              emphasis: settings.bass === "emphasis",
              playbackRate: peer.motion?.tempoRate ?? peer.motion?.rate,
              beats: peer.analysis.beats,
            },
          );
        } else {
          const master = decks.findIndex((deck) => deck?.live?.master === true);
          const order = master < 0 ? [0, 1] : [master, 1 - master];
          const labelWidth = 86;
          const anchor = width * (settings.playhead === "third" ? 1 / 3 : 0.5);
          // Match the main waveform's playback-time scale, including tempo changes.
          const pixelsPerSecond = width / settings.window;
          const upper = 14,
            lower = height - 14,
            middle = (upper + lower) / 2;
          const orange = "#ffac24",
            blue = "#29b6ed",
            white = "#eeeeee",
            red = "#f0313b";
          ctx.font = "11px Arial";
          order.forEach((deckIndex, row) => {
            ctx.fillStyle = row === 0 ? orange : blue;
            ctx.fillText(
              `CDJ ${decks[deckIndex]?.live?.number ?? deckIndex + 1}${deckIndex === master ? " MASTER" : ""}`,
              3,
              (row === 0 ? upper : lower) + 4,
            );
            ctx.fillRect(
              labelWidth,
              row === 0 ? upper : lower,
              width - labelWidth,
              2,
            );
          });
          ctx.save();
          ctx.beginPath();
          ctx.rect(labelWidth, 0, width - labelWidth, height);
          ctx.clip();
          order.forEach((deckIndex, row) => {
            const deck = decks[deckIndex]!;
            const laneRate =
              Math.abs(deck.motion?.tempoRate ?? deck.motion?.rate ?? 1) || 1;
            const grid = deck.analysis.beats!;
            const secondsBefore = (anchor / pixelsPerSecond) * laneRate;
            const secondsAfter =
              ((width - anchor) / pixelsPerSecond) * laneRate;
            // Each lane uses its own beatgrid, so unequal tempos and variable grids remain visible.
            const start =
              beatAt(positions[deckIndex] - secondsBefore, grid)?.index ?? 0;
            for (let b = start; b < grid.length; b++) {
              const delta = grid[b].time - positions[deckIndex];
              if (delta > secondsAfter) break;
              const x = anchor + (delta / laneRate) * pixelsPerSecond;
              if (x < labelWidth) continue;
              const downbeat = grid[b].beatInBar === 1;
              ctx.fillStyle = downbeat ? red : white;
              const top = row === 0 ? (downbeat ? upper - 6 : upper) : middle;
              const bottom =
                row === 0 ? middle : downbeat ? lower + 8 : lower + 2;
              ctx.fillRect(
                Math.round(x),
                top,
                downbeat ? 2.5 : 1.5,
                bottom - top,
              );
            }
          });
          ctx.fillStyle = white;
          ctx.fillRect(Math.round(anchor), 3, 2, height - 6);
          ctx.restore();
        }
      }
      frame = requestAnimationFrame(render);
    };
    frame = requestAnimationFrame(render);
    return () => cancelAnimationFrame(frame);
  }, []);
  const other = decks[1 - index];
  return (
    <div className="deck-comparison">
      <button
        className="comparison-toggle"
        onClick={onToggle}
        title="Switch between beat phase and reference waveform"
      >
        <span>
          {settings.phaseDisplay === "phase"
            ? "BEAT PHASE"
            : `REFERENCE · CDJ ${other?.live?.number ?? 2 - index}`}
        </span>
        <span>
          {settings.phaseDisplay === "phase"
            ? "Show reference waveform"
            : "Show beat phase"}
        </span>
      </button>
      <canvas
        ref={ref}
        role="img"
        aria-label={
          settings.phaseDisplay === "phase"
            ? "CDJ beat phase comparison"
            : "Other CDJ reference waveform"
        }
      />
    </div>
  );
}
