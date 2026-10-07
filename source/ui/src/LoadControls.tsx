import { useEffect, useRef, useState } from "react";
import type { LibrarySource, LibraryTrack } from "./libraryModel";
import type { LivePlayer } from "./model";
import { unsupportedReason } from "./trackCompatibility";
import { loadBlocked } from "./libraryLoading";
export function useTrackLoader(
  source: LibrarySource | undefined,
  active: boolean,
  onLoaded: () => void,
) {
  const [busy, setBusy] = useState<number | null>(null);
  const pending = useRef(false);
  const [message, setMessage] = useState("");
  const key = `${source?.id}:${source?.generation}:${active}`;
  const currentKey = useRef(key);
  currentKey.current = key;
  useEffect(() => {
    setMessage("");
  }, [key]);
  async function send(track: LibraryTrack, target: number) {
    if (!source || pending.current || track.available === false) return;
    const requestKey = key;
    pending.current = true;
    setBusy(target);
    setMessage(`Loading ${track.title} to CDJ${target}…`);
    try {
      const response = await fetch("/api/live/load", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          source: source.id,
          generation: source.generation,
          trackId: track.id,
          target,
        }),
      });
      if (!response.ok) throw new Error(await response.text());
      const result = (await response.json()) as {
        message: string;
        outcome: string;
      };
      if (currentKey.current === requestKey) {
        setMessage(`CDJ${target}: ${result.message}`);
        if (result.outcome === "confirmed" || result.outcome === "reported")
          onLoaded();
      }
    } catch {
      if (currentKey.current === requestKey)
        setMessage(
          "Connection lost while requesting a load. Check the CDJ before trying again; no automatic retry was sent.",
        );
    } finally {
      pending.current = false;
      setBusy(null);
    }
  }
  return { busy, message, send };
}
export function LoadControls({
  track,
  source,
  players,
  loader,
}: {
  track: LibraryTrack;
  source: LibrarySource | undefined;
  players: LivePlayer[];
  loader: ReturnType<typeof useTrackLoader>;
}) {
  return (
    <div className="library-row-load">
      {[1, 2].map((number) => {
        const player = players.find((p) => p.number === number);
        const connected = player?.connection === "connected";
        const blocked = loadBlocked(player);
        const unsupported = unsupportedReason(track, player);
        return (
          <button
            key={number}
            className={blocked || unsupported ? "playing" : ""}
            disabled={
              track.available === false ||
              !source?.loadable ||
              !source.available ||
              !connected ||
              blocked ||
              unsupported !== null ||
              loader.busy !== null
            }
            title={
              unsupported ??
              (track.available === false
                ? "Track is missing or ambiguous on the selected USB"
                : blocked
                  ? `CDJ${number} is playing or looping — stop playback before loading`
                  : !connected
                    ? `CDJ${number} is not connected`
                    : `Load ${track.title} to CDJ${number}`)
            }
            onClick={() => void loader.send(track, number)}
          >
            CDJ{number}
          </button>
        );
      })}
    </div>
  );
}
