import { useState } from "react";
import type { LibraryTrack } from "./libraryModel";
export function LibraryArtwork({
  track,
  base,
  generation,
  large = false,
}: {
  track: LibraryTrack;
  base: string;
  generation: number | undefined;
  large?: boolean;
}) {
  const [failed, setFailed] = useState(false);
  const src = track.savedArtwork
    ? `/api/sets/artwork/${encodeURIComponent(String(track.savedArtwork))}`
    : track.artworkPath && generation !== undefined
      ? `${base}/artwork/${track.id}?generation=${generation}`
      : "";
  return (
    <div className={`library-artwork ${large ? "large" : ""}`}>
      {src && !failed ? (
        <img
          src={String(src)}
          alt={large ? `Artwork for ${track.title}` : ""}
          loading={large ? "eager" : "lazy"}
          decoding="async"
          onError={() => setFailed(true)}
        />
      ) : (
        <span role="img" aria-label="No artwork">
          ♪
        </span>
      )}
    </div>
  );
}
