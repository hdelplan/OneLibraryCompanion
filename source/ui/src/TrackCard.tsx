import type { ReactNode } from "react";
import type { Deck } from "./model";
import { durationOf } from "./model";
import type { Settings } from "./settings";
import { timeLabel } from "./settings";
import { PlayerTime } from "./PlayerTime";
import { Signal } from "./Signal";
export function TrackCard({
  deck,
  i,
  settings,
  onToggleTime,
  browserPlayer,
}: {
  deck: Deck;
  i: number;
  settings: Settings;
  onToggleTime: () => void;
  browserPlayer?: {
    control: ReactNode;
    artwork: ReactNode;
    metadata: ReactNode;
    titleId: string;
    onSeek: (position: number) => void;
  };
}) {
  const track = deck?.analysis.track,
    duration = durationOf(deck),
    position = deck?.position ?? null;
  const live = deck?.live;
  const bpm = live ? live.bpm : track?.bpm;
  return (
    <section
      className="track-card"
      aria-label={
        browserPlayer
          ? "Browser track information"
          : `Deck ${i + 1} track information`
      }
    >
      <header>
        {browserPlayer ? (
          <div className="deck-number">{browserPlayer.control}</div>
        ) : (
          <div className="deck-number">
            <small>DECK</small>
            <b>{live?.number ?? i + 1}</b>
          </div>
        )}
        <div className="track-title">
          <strong id={browserPlayer?.titleId}>
            {track?.title || deck?.name || "No track loaded"}
          </strong>
          <span>
            {track?.artist ||
              (deck
                ? live
                  ? live.name
                  : "Saved analysis · offline preview"
                : "Track information will appear here")}
          </span>
        </div>
      </header>
      <div className="track-metrics">
        <div className="artwork-box">
          {browserPlayer ? (
            browserPlayer.artwork
          ) : deck?.analysis.artworkAvailable && live?.trackKey ? (
            <img
              key={live.trackKey}
              src={`/api/live/artwork/${live.number}?key=${encodeURIComponent(live.trackKey)}`}
              alt="Track artwork"
            />
          ) : (
            <span className="artwork-placeholder" aria-label="No artwork">
              ♪
            </span>
          )}
        </div>
        <button
          type="button"
          className="time-box"
          onClick={onToggleTime}
          aria-label={`Time display: ${settings.time}. Switch to ${settings.time === "remaining" ? "elapsed" : "remaining"}`}
          title="Toggle remaining / elapsed time"
        >
          <small>{settings.time === "remaining" ? "REMAIN" : "ELAPSED"}</small>
          <PlayerTime
            position={position}
            duration={duration}
            remaining={settings.time === "remaining"}
            milliseconds
            motion={deck?.motion}
          />
          <span>
            {settings.time === "remaining" ? "Elapsed" : "Total"}{" "}
            {timeLabel(
              settings.time === "remaining"
                ? deck
                  ? position
                  : null
                : duration,
            )}
          </span>
        </button>
        <div className="standard-bpm-box">
          <small>BPM</small>
          <strong>{track?.bpm ? track.bpm.toFixed(2) : "—"}</strong>
        </div>
        <div className="pitch-box">
          <small>{browserPlayer ? "KEY" : "PITCH"}</small>
          <strong>
            {browserPlayer
              ? track?.key || "—"
              : live?.pitch == null
                ? "—"
                : `${live.pitch >= 0 ? "+" : ""}${live.pitch.toFixed(2)}`}
            {!browserPlayer && <em>%</em>}
          </strong>
        </div>
        <div className="bpm-box">
          <strong>{bpm ? bpm.toFixed(2) : "—"}</strong>
          <span className="master-badge">{live?.master ? "MASTER" : ""}</span>
        </div>
      </div>
      {settings.details && (
        <div className="track-details">
          <span>
            ALBUM <b>{track?.album || "—"}</b>
          </span>
          <span>
            LENGTH <b>{timeLabel(duration)}</b>
          </span>
        </div>
      )}
      {browserPlayer?.metadata}
      {settings.overview && (
        <div
          className={`overview${browserPlayer ? " browser-player-overview" : ""}`}
          onClick={
            browserPlayer && duration
              ? (event) => {
                  const rect = event.currentTarget.getBoundingClientRect();
                  browserPlayer.onSeek(
                    Math.max(
                      0,
                      Math.min(1, (event.clientX - rect.left) / rect.width),
                    ) * duration,
                  );
                }
              : undefined
          }
        >
          <Signal
            wave={
              deck?.analysis.preview ??
              (browserPlayer ? deck?.analysis.detail : null) ??
              null
            }
            position={position ?? 0}
            overview
            motion={deck?.motion}
            cues={[
              ...(deck?.analysis.cues ?? []),
              ...(deck?.live?.currentCue != null
                ? [
                    {
                      time: deck.live.currentCue,
                      hot: 0,
                      label: "CUE",
                      color: "#ffffff",
                    },
                  ]
                : []),
            ]}
            loop={live?.connection === "connected" ? live.loop : null}
            duration={position === null ? 0 : (duration ?? 0)}
            settings={settings}
          />
          {!deck?.analysis.preview &&
            !(browserPlayer && deck?.analysis.detail) && (
              <span>No overview data</span>
            )}
        </div>
      )}
      {settings.phrases && (
        <div className="phrases" aria-label="Track phrase analysis">
          {deck?.analysis.phrases?.length && duration ? (
            deck.analysis.phrases.map((phrase, index) => (
              <div
                key={index}
                className="phrase"
                style={{
                  backgroundColor: phrase.color ?? "#555555",
                  color: phrase.textColor ?? "#ffffff",
                  left: `${(100 * phrase.start) / duration}%`,
                  width: `${(100 * (phrase.end - phrase.start)) / duration}%`,
                }}
                title={`${phrase.label} · ${timeLabel(phrase.start)}–${timeLabel(phrase.end)}`}
              >
                <span>{phrase.label.toUpperCase()}</span>
              </div>
            ))
          ) : (
            <span className="phrases-empty">
              {deck
                ? (deck.analysis.phraseWarning ?? "Phrase analysis unavailable")
                : "No phrase data"}
            </span>
          )}
        </div>
      )}
    </section>
  );
}
