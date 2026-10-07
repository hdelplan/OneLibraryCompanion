import { SyncStatus } from "./SyncStatus";
import { useState } from "react";
import { TrackInfoPopup } from "./TrackInfoPopup";
import { CueCountdown } from "./CueCountdown";
import { TrackCard } from "./TrackCard";
import type { Deck } from "./model";
import type { Settings } from "./settings";
import { Signal } from "./Signal";
export function StatusDisplay({
  decks,
  settings,
  onToggleTime,
  onZoom,
}: {
  decks: [Deck, Deck];
  settings: Settings;
  onToggleTime: () => void;
  onZoom: (window: number) => void;
}) {
  const [infoDeck, setInfoDeck] = useState<number | null>(null);
  return (
    <>
      <div className="waveforms">
        {decks.map((deck, i) => {
          const track = deck?.analysis.track;
          return (
            <section
              className="wave-row"
              key={i}
              aria-label={`Deck ${i + 1} waveform`}
            >
              <aside className="rail">
                <h2>DECK {deck?.live?.number ?? i + 1}</h2>
                <div className="source-label">
                  {deck?.live
                    ? (
                        deck.live.sourceLabel ?? deck.live.connection
                      ).toUpperCase()
                    : deck
                      ? "SAVED DATA"
                      : "NO SOURCE"}
                </div>
                <div className="rail-key">
                  <small>KEY</small>
                  <b>{track?.key || "—"}</b>
                </div>
                <div className="rail-state">
                  <small>PLAY STATE</small>
                  <b>
                    {deck?.live
                      ? ((deck.live.connection === "connected"
                          ? deck.live.playState === "looping" && deck.live.loop
                            ? `Looping (${deck.live.loop.beats})`
                            : deck.live.playState
                          : deck.live.connection
                        )?.toUpperCase() ?? "—")
                      : "—"}
                  </b>
                </div>
                <div className="rail-tags">
                  <SyncStatus
                    decks={decks}
                    index={i}
                    smoothMs={settings.jogSmoothing}
                  />
                </div>
              </aside>
              <div className="signal">
                <button
                  className="track-info-button"
                  aria-label={`Track information for deck ${deck?.live?.number ?? i + 1}`}
                  disabled={!track}
                  onClick={() => setInfoDeck(i)}
                >
                  i
                </button>
                <Signal
                  wave={
                    deck?.source === "live" && deck.position === null
                      ? null
                      : (deck?.analysis.detail ?? null)
                  }
                  position={deck?.position ?? 0}
                  settings={settings}
                  beats={deck?.analysis.beats}
                  onZoom={onZoom}
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
                  loop={
                    deck?.live?.connection === "connected"
                      ? deck.live.loop
                      : null
                  }
                />
                <CueCountdown deck={deck} smoothMs={settings.jogSmoothing} />
                {!deck && (
                  <div className="empty">
                    <strong>Waiting for CDJ {i + 1}</strong>
                    <span>
                      Choose an offline preview in{" "}
                      {__OLC_EXPERIMENTS__ ? "TEST" : "MENU"}
                    </span>
                  </div>
                )}
                {deck?.live &&
                  (!deck.analysis.detail || deck.position === null) && (
                    <div className="empty">
                      <strong>
                        {deck.live.connection !== "connected"
                          ? deck.live.connection
                          : deck.live.trackKey
                            ? "Track analysis"
                            : "No track loaded"}
                      </strong>
                      <span>
                        {deck.live.warning ??
                          (deck.live.trackKey
                            ? deck.live.assetReady
                              ? "Position unavailable"
                              : "Reading USB analysis…"
                            : "Load a track on the CDJ")}
                      </span>
                    </div>
                  )}
              </div>
            </section>
          );
        })}
        {infoDeck !== null && (
          <TrackInfoPopup
            deck={decks[infoDeck]}
            onClose={() => setInfoDeck(null)}
          />
        )}
      </div>
      <div className={`track-cards ${settings.overview ? "" : "no-overview"}`}>
        {decks.map((deck, i) => (
          <TrackCard
            key={i}
            deck={deck}
            i={i}
            settings={settings}
            onToggleTime={onToggleTime}
          />
        ))}
      </div>
    </>
  );
}
