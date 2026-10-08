import { useEffect, useRef, useState } from "react";
import type { LibrarySource, LibraryTrack } from "./libraryModel";
import type { LivePlayer } from "./model";
import {
  conversionId,
  conversionMessage,
  type ConversionState,
  type ConversionJob,
} from "./Transcoding";
import { canTranscode, unsupportedReason } from "./trackCompatibility";
import { loadBlocked } from "./libraryLoading";
export function useTrackLoader(
  source: LibrarySource | undefined,
  active: boolean,
  onLoaded: () => void,
) {
  const [busy, setBusy] = useState<number | null>(null);
  const pending = useRef(false);
  const [cancelJob, setCancelJob] = useState<string | null>(null);
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
    const jobId = conversionId();
    let polling = true;
    let conversion = "";
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try {
        const r = await fetch("/api/transcoding");
        if (r.ok) {
          const state = (await r.json()) as ConversionState;
          const job = state.jobs[jobId];
          if (job && polling && currentKey.current === requestKey) {
            setCancelJob(job.state === "transcoding" ? jobId : null);
            conversion = conversionMessage(job);
            setMessage(
              `${track.title}: ${conversion}${job.state === "ready" ? ` · Loading to CDJ${target}…` : ""}`,
            );
          }
        }
      } catch {
        /* The load response reports connection failures. */
      }
      if (polling) timer = setTimeout(() => void poll(), 400);
    }
    if (source.id.startsWith("local-usb:")) void poll();
    try {
      const response = await fetch("/api/live/load", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          source: source.id,
          generation: source.generation,
          trackId: track.id,
          target,
          conversionJob: jobId,
        }),
      });
      if (!response.ok) {
        const detail = await response.text();
        if (currentKey.current === requestKey)
          setMessage(detail || `Load request rejected (${response.status})`);
        return;
      }
      const result = (await response.json()) as {
        message: string;
        outcome: string;
        elapsedSeconds?: number;
        conversion?: ConversionJob | null;
      };
      if (currentKey.current === requestKey) {
        if (result.conversion)
          conversion = conversionMessage(result.conversion);
        setMessage(
          `CDJ${target}: ${result.message}${conversion ? ` · ${conversion}` : ""}${result.elapsedSeconds !== undefined ? ` · Total ${result.elapsedSeconds.toFixed(2)} s` : ""}`,
        );
        if (result.outcome === "confirmed" || result.outcome === "reported")
          onLoaded();
      }
    } catch {
      if (currentKey.current === requestKey)
        setMessage(
          "Connection lost while requesting a load. Check the CDJ before trying again; no automatic retry was sent.",
        );
    } finally {
      polling = false;
      clearTimeout(timer!);
      pending.current = false;
      setBusy(null);
      setCancelJob(null);
    }
  }
  async function cancel() {
    if (!cancelJob) return;
    try {
      await fetch("/api/transcoding/cancel", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ id: cancelJob }),
      });
    } catch {
      setMessage("Could not cancel conversion. Check the CDJ before retrying.");
    }
  }
  return { busy, message, send, cancelJob, cancel };
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
        const transcode =
          unsupported !== null && canTranscode(track, source?.id, player);
        return (
          <button
            key={number}
            className={
              blocked || (unsupported && !transcode)
                ? "playing"
                : transcode
                  ? "transcode-needed"
                  : ""
            }
            disabled={
              track.available === false ||
              !source?.loadable ||
              !source.available ||
              !connected ||
              blocked ||
              (unsupported !== null && !transcode) ||
              loader.busy !== null
            }
            title={
              (!source?.loadable
                ? (source?.loadUnavailableReason ??
                  "This library supports browsing only; CDJ loading is not yet available")
                : transcode
                  ? `${unsupported}. OLC will convert the audio locally before loading.`
                  : unsupported) ??
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
