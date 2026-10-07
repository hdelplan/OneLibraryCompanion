import { useEffect, useState } from "react";
import type { Deck } from "./model";

export function TrackInfoPopup({
  deck,
  onClose,
}: {
  deck: Deck;
  onClose: () => void;
}) {
  const track = deck?.analysis.track;
  const [failedArtwork, setFailedArtwork] = useState(false);
  useEffect(() => {
    const dismiss = () => onClose();
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("click", dismiss, true);
    document.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("click", dismiss, true);
      document.removeEventListener("keydown", key);
    };
  }, [onClose]);
  const artwork =
    deck?.analysis.artworkUrl ??
    (deck?.analysis.artworkAvailable && deck.live?.trackKey
      ? `/api/live/artwork/${deck.live.number}?key=${encodeURIComponent(deck.live.trackKey)}`
      : "");
  const duration = track?.duration;
  const time =
    duration != null && Number.isFinite(duration) && duration >= 0
      ? `${Math.floor(duration / 60)
          .toString()
          .padStart(2, "0")}:${Math.floor(duration % 60)
          .toString()
          .padStart(2, "0")}`
      : "—";
  const fields = [
    ["BPM", track?.bpm ? track.bpm.toFixed(2) : "—"],
    ["Key", track?.key],
    ["Duration", time],
    [
      "Audio format",
      [
        track?.format?.toUpperCase(),
        track?.sampleRate ? `${track.sampleRate / 1000} kHz` : "",
        track?.sampleDepth ? `${track.sampleDepth}-bit` : "",
      ]
        .filter(Boolean)
        .join(" · "),
    ],
    ...(track?.databaseFormat ? [["Library", track.databaseFormat]] : []),
    ...(track?.audioHeader
      ? [
          [
            "WAV file header",
            `${track.audioHeader.encoding === 1 ? "PCM" : `Encoding ${track.audioHeader.encoding}`} · ${track.audioHeader.sampleRate / 1000} kHz · ${track.audioHeader.sampleDepth}-bit · ${track.audioHeader.channels} channels`,
            "wide",
          ],
          [
            "WAV layout",
            `Audio offset ${track.audioHeader.dataOffset} bytes · frame size ${track.audioHeader.blockAlign} bytes`,
            "wide",
          ],
        ]
      : []),
    ["Date added", track?.dateAdded],
    ["Genre", track?.genre],
    ["Mood", deck?.analysis.phraseMood ?? "Unavailable"],
    [
      "Rating",
      track?.rating == null
        ? "—"
        : "★".repeat(Math.max(0, Math.min(5, Math.round(track.rating)))) +
          "☆".repeat(5 - Math.max(0, Math.min(5, Math.round(track.rating)))),
    ],
    ["Color", track?.color],
    ["Album", track?.album],
    [
      "MyTag",
      track?.myTags == null
        ? "Unavailable"
        : track.myTags.map((tag) => tag.name).join(" · ") || "None",
      "wide",
    ],
    ["Comment", track?.comment, "wide"],
  ];
  return (
    <>
      <div className="track-info-dismiss" onClick={onClose} />
      <section
        className="track-info-popup"
        role="dialog"
        aria-modal="true"
        aria-label="Track information"
        onClick={onClose}
      >
        <div className="track-info-heading">
          <div className="track-info-art">
            {artwork && !failedArtwork ? (
              <img
                src={artwork}
                alt="Track artwork"
                onError={() => setFailedArtwork(true)}
              />
            ) : (
              <span aria-label="No artwork">♪</span>
            )}
          </div>
          <div>
            <small>
              TRACK INFO · {deck?.live ? `CDJ ${deck.live.number}` : "PREVIEW"}
            </small>
            <h2>{track?.title || "Unknown title"}</h2>
            <p>{track?.artist || "Unknown artist"}</p>
          </div>
        </div>
        <dl className="track-info-fields">
          {fields.map(([label, value, wide]) => (
            <div key={label} className={wide ? "track-info-wide" : undefined}>
              <dt>{label}</dt>
              <dd>{value || "—"}</dd>
            </div>
          ))}
        </dl>
        <small>Tap anywhere to dismiss</small>
      </section>
    </>
  );
}
