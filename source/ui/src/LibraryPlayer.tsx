import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { LibraryArtwork } from "./LibraryArtwork";
import { colorPalette, type LibraryTrack } from "./libraryModel";
import type { Analysis } from "./model";
import { TrackCard } from "./TrackCard";
import { readSettings } from "./settings";

export function LibraryPlayer({
  track,
  base,
  generation,
  onClose,
}: {
  track: LibraryTrack;
  base: string;
  generation: number;
  onClose: () => void;
}) {
  const audio = useRef<HTMLAudioElement>(null);
  const close = useRef<HTMLButtonElement>(null);
  const [analysis, setAnalysis] = useState<Analysis | null>(null);
  const [waveError, setWaveError] = useState("");
  const [error, setError] = useState("");
  const [playing, setPlaying] = useState(false);
  const [preparing, setPreparing] = useState(true);
  const [position, setPosition] = useState(0);
  const [length, setLength] = useState(track.duration);
  const [settings, setSettings] = useState(readSettings);
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    close.current?.focus();
    const controller = new AbortController();
    fetch(`${base}/preview/${track.id}?generation=${generation}`, {
      signal: controller.signal,
    })
      .then(async (r) => {
        if (!r.ok) throw new Error(await r.text());
        return r.json() as Promise<Analysis>;
      })
      .then(setAnalysis)
      .catch((e) => {
        if (!controller.signal.aborted) setWaveError(String(e));
      });
    const player = audio.current;
    return () => {
      controller.abort();
      player?.pause();
      if (player) {
        player.removeAttribute("src");
        player.load();
      }
      previous?.focus();
    };
  }, [base, generation, track.id]);
  async function play() {
    if (!audio.current) return;
    if (playing) audio.current.pause();
    else {
      setError("");
      try {
        await audio.current.play();
      } catch {
        setError(
          "Playback unavailable. Check the source connection and browser support for this audio format.",
        );
      }
    }
  }
  function seek(value: number) {
    if (!audio.current || !Number.isFinite(value)) return;
    audio.current.currentTime = value;
    setPosition(value);
  }
  return createPortal(
    <div
      className="library-player-backdrop"
      onKeyDown={(e) => {
        if (e.key === "Escape") onClose();
        if (e.key === "Tab") {
          const items = Array.from(
            e.currentTarget.querySelectorAll<HTMLElement>(
              "button:not(:disabled), input",
            ),
          );
          const index = items.indexOf(document.activeElement as HTMLElement);
          if (
            (e.shiftKey && index === 0) ||
            (!e.shiftKey && index === items.length - 1)
          ) {
            e.preventDefault();
            items[e.shiftKey ? items.length - 1 : 0]?.focus();
          }
        }
      }}
    >
      <section
        className="library-player"
        role="dialog"
        aria-modal="true"
        aria-labelledby="library-player-title"
      >
        <button
          ref={close}
          className="library-player-close"
          onClick={onClose}
          aria-label="Close player"
          title="Close player"
        >
          <span aria-hidden="true">×</span>
        </button>
        <TrackCard
          deck={{
            name: track.title,
            source: "offline",
            position,
            analysis: {
              ...(analysis ?? { detail: null, preview: null }),
              track: {
                ...track,
                bitrate: Number(track.bitrate) || 0,
                sampleRate: Number(track.sampleRate) || 0,
                duration: length,
              },
            },
          }}
          i={0}
          settings={{
            ...settings,
            details: true,
            overview: true,
            phrases: true,
          }}
          onToggleTime={() =>
            setSettings((previous) => ({
              ...previous,
              time: previous.time === "remaining" ? "elapsed" : "remaining",
            }))
          }
          browserPlayer={{
            titleId: "library-player-title",
            control: (
              <button
                className={`library-transport${playing ? " is-playing" : ""}`}
                onClick={() => void play()}
                aria-label={playing ? "Pause track" : "Play track"}
                aria-pressed={playing}
                title={playing ? "Pause" : "Play"}
              >
                <span className="library-transport-symbols" aria-hidden="true">
                  <span className="library-transport-play" />
                  <span className="library-transport-divider">/</span>
                  <span className="library-transport-pause">
                    <span />
                    <span />
                  </span>
                </span>
              </button>
            ),
            artwork: (
              <LibraryArtwork
                track={track}
                base={base}
                generation={generation}
              />
            ),
            metadata: (
              <div className="browser-player-metadata">
                <div>
                  <small>GENRE</small>
                  <span>{track.genre || "—"}</span>
                  <span
                    className="library-player-color"
                    style={{
                      background:
                        colorPalette[track.colorId] ?? colorPalette[0],
                    }}
                    aria-label={track.color || "No color"}
                    title={track.color || "No color"}
                  />
                </div>
                <div>
                  <small>MY TAGS</small>
                  <span>
                    {track.myTags?.map((tag) => tag.name).join(" · ") || "—"}
                  </span>
                </div>
                <div>
                  <small>COMMENTS</small>
                  <span>{String(track.comment || "—")}</span>
                </div>
              </div>
            ),
            onSeek: seek,
          }}
        />
        <audio
          ref={audio}
          preload="metadata"
          src={`${base}/audio/${track.id}?generation=${generation}`}
          onPlay={() => setPlaying(true)}
          onPause={() => setPlaying(false)}
          onEnded={() => setPlaying(false)}
          onTimeUpdate={() => setPosition(audio.current?.currentTime ?? 0)}
          onLoadedMetadata={() => {
            setPreparing(false);
            if (audio.current && Number.isFinite(audio.current.duration))
              setLength(audio.current.duration);
          }}
          onError={() => {
            setPreparing(false);
            setError(
              "Playback unavailable. Check the source connection and browser support for this audio format.",
            );
          }}
        />
        {preparing && <p role="status">Preparing audio…</p>}
        {error && <p role="alert">{error}</p>}
        {waveError && <p role="status">Waveform unavailable.</p>}
      </section>
    </div>,
    document.body,
  );
}
