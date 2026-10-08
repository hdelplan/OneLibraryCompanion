import { useEffect, useState } from "react";
import type { LibrarySource, LibraryTrack, TrackPage } from "./libraryModel";
import type { LivePlayer } from "./model";

export type ConversionJob = {
  state: "queued" | "transcoding" | "ready" | "error";
  title?: string;
  profile?: string;
  progress?: number;
  seconds?: number;
  cacheHit?: boolean;
  benchmark?: boolean;
  message?: string;
  result?: {
    bytes: number;
    sampleRate: number;
    sampleDepth: number;
    frames: number;
    clippedSamples: number;
  };
};
export type ConversionState = {
  settings: { profile: string };
  profiles: { id: string; label: string }[];
  jobs: Record<string, ConversionJob>;
  cacheBytes: number;
  engine: string;
};
export const conversionId = () =>
  `convert-${Date.now()}-${Math.random().toString(36).slice(2)}`;
export function conversionMessage(job: ConversionJob): string {
  if (job.state === "error") return job.message ?? "Conversion failed";
  if (job.state === "queued") return "Preparing audio…";
  if (job.state === "transcoding")
    return `Transcoding to ${job.profile} · ${Math.round((job.progress ?? 0) * 100)}% · ${(job.seconds ?? 0).toFixed(1)} s`;
  return job.cacheHit
    ? `Cached ${job.profile}`
    : `${job.profile} converted in ${(job.seconds ?? 0).toFixed(2)} s${job.result?.clippedSamples ? ` · ${job.result.clippedSamples} clipped samples` : ""}`;
}
async function api<T>(
  url: string,
  body?: unknown,
  signal?: AbortSignal,
): Promise<T> {
  const r = await fetch(url, {
    signal,
    ...(body === undefined
      ? {}
      : {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        }),
  });
  if (!r.ok) throw Error(await r.text());
  return r.json() as Promise<T>;
}
export function TranscodingSettings({ players }: { players: LivePlayer[] }) {
  const [state, setState] = useState<ConversionState | null>(null);
  const [sources, setSources] = useState<LibrarySource[]>([]);
  const [sourceId, setSourceId] = useState("");
  const [tracks, setTracks] = useState<LibraryTrack[]>([]);
  const [trackId, setTrackId] = useState(0);
  const [search, setSearch] = useState("");
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  const [running, setRunning] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const source = sources.find((s) => s.id === sourceId);
  useEffect(() => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try {
        const [next, list] = await Promise.all([
          api<ConversionState>(
            "/api/transcoding",
            undefined,
            controller.signal,
          ),
          api<{ sources: LibrarySource[] }>(
            "/api/library/sources",
            undefined,
            controller.signal,
          ),
        ]);
        if (controller.signal.aborted) return;
        setState(next);
        const local = list.sources.filter(
          (s) => s.id.startsWith("local-usb:") && s.available,
        );
        setSources(local);
        setSourceId((old) =>
          local.some((s) => s.id === old) ? old : (local[0]?.id ?? ""),
        );
        setRunning((old) =>
          old && ["ready", "error"].includes(next.jobs[old]?.state)
            ? null
            : old,
        );
      } catch (e) {
        if (!controller.signal.aborted) setError(String(e));
      }
      if (!controller.signal.aborted)
        timer = setTimeout(() => void poll(), 750);
    }
    void poll();
    return () => {
      controller.abort();
      clearTimeout(timer);
    };
  }, []);
  useEffect(() => {
    setTracks([]);
    setTrackId(0);
    if (!source) return;
    const controller = new AbortController();
    const query = new URLSearchParams({
      generation: String(source.generation),
      q: search,
      limit: "100",
    });
    const timer = setTimeout(() => {
      void api<TrackPage>(
        `/api/library/${encodeURIComponent(source.id)}/tracks?${query}`,
        undefined,
        controller.signal,
      )
        .then((page) => {
          setTracks(page.tracks);
          setTrackId(page.tracks[0]?.id ?? 0);
        })
        .catch((e) => {
          if (!controller.signal.aborted) setError(String(e));
        });
    }, 250);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, [source?.id, source?.generation, search]);
  const active = Object.values(state?.jobs ?? {}).some(
    (j) => j.state === "queued" || j.state === "transcoding",
  );
  const results = Object.entries(state?.jobs ?? {})
    .filter(([, j]) => j.benchmark)
    .reverse()
    .slice(0, 8);
  return (
    <section className="settings-panel transcoding-settings">
      <h2>Audio transcoding</h2>
      <p>
        Unsupported local USB audio is converted on this device. Compatible
        tracks use the original. USB files are never changed.
      </p>
      <label>
        Target format
        <select
          value={state?.settings.profile ?? "auto"}
          disabled={!state || saving}
          onChange={async (e) => {
            setSaving(true);
            setError("");
            try {
              setState(
                await api<ConversionState>("/api/transcoding", {
                  profile: e.target.value,
                }),
              );
            } catch (e) {
              setError(String(e));
            } finally {
              setSaving(false);
            }
          }}
        >
          {state?.profiles.map((p) => (
            <option key={p.id} value={p.id}>
              {p.label}
            </option>
          ))}
        </select>
      </label>
      <p>
        WAV and AIFF targets use stereo 44.1 or 48 kHz. Automatic preserves
        16/24-bit depth where possible. Higher rates are resampled.
      </p>
      <p>
        {players
          .filter((p) => p.connection === "connected")
          .map((p) => `CDJ${p.number}: ${p.name}`)
          .join(" · ") ||
          "Connect players to check the destination at load time."}{" "}
        Supported conversion destinations: CDJ-2000nexus, CDJ-2000NXS2 and
        CDJ-3000.
      </p>
      <h3>Conversion benchmark</h3>
      <p>
        Converts a complete track using the selected target, bypassing the
        conversion cache. Does not load or play a CDJ. Repeat with each format
        on each host; USB reads are included.
      </p>
      <label>
        Local USB{" "}
        <select
          value={sourceId}
          disabled={!!running || starting}
          onChange={(e) => setSourceId(e.target.value)}
        >
          {!sources.length && <option value="">Connect a local USB</option>}
          {sources.map((s) => (
            <option key={s.id} value={s.id}>
              {s.label}
            </option>
          ))}
        </select>
      </label>
      {source?.loadUnavailableReason && (
        <p>
          {source.loadUnavailableReason} Conversion benchmarks are still
          available.
        </p>
      )}
      <label>
        Find a track{" "}
        <input
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="Title or artist"
        />
      </label>
      <label>
        Track{" "}
        <select
          value={trackId}
          disabled={!!running || starting || !tracks.length}
          onChange={(e) => setTrackId(Number(e.target.value))}
        >
          {!tracks.length && <option value={0}>No matching tracks</option>}
          {tracks.map((t) => (
            <option key={t.id} value={t.id}>
              {t.artist} — {t.title} · {String(t.format ?? "")}
            </option>
          ))}
        </select>
      </label>
      <button
        disabled={
          !source ||
          !trackId ||
          !state ||
          !!running ||
          starting ||
          active ||
          saving
        }
        onClick={async () => {
          if (!source || !state) return;
          setStarting(true);
          setError("");
          try {
            const result = await api<{ id: string }>(
              "/api/transcoding/benchmark",
              {
                source: source.id,
                generation: source.generation,
                trackId,
                profile: state.settings.profile,
                conversionJob: conversionId(),
              },
            );
            setRunning(result.id);
          } catch (e) {
            setError(String(e));
          } finally {
            setStarting(false);
          }
        }}
      >
        {starting || running ? "Benchmark running…" : "Run benchmark"}
      </button>
      {running && (
        <button
          onClick={() =>
            void api("/api/transcoding/cancel", { id: running }).catch((e) =>
              setError(String(e)),
            )
          }
        >
          Cancel benchmark
        </button>
      )}
      <div aria-live="polite">
        {results.map(([id, job]) => (
          <p key={id}>
            <b>{job.title || "Preparing track"}</b>
            <br />
            {conversionMessage(job)}
            {job.result && (
              <>
                {" "}
                · {(job.result.bytes / 1e6).toFixed(1)} MB ·{" "}
                {job.result.sampleRate / 1000} kHz / {job.result.sampleDepth}
                -bit
                {job.result.clippedSamples > 0 && (
                  <> · {job.result.clippedSamples} clipped samples</>
                )}
              </>
            )}
          </p>
        ))}
      </div>
      {results.length > 0 && (
        <button
          onClick={async () => {
            try {
              const file = new File(
                [JSON.stringify(state, null, 2)],
                "olc-transcoding-results.json",
                { type: "application/json" },
              );
              if (navigator.canShare?.({ files: [file] }))
                await navigator.share({
                  files: [file],
                  title: "OLC conversion benchmark",
                });
              else {
                const url = URL.createObjectURL(file);
                const a = document.createElement("a");
                a.href = url;
                a.download = file.name;
                a.click();
                setTimeout(() => URL.revokeObjectURL(url), 60000);
              }
            } catch (e) {
              setError(String(e));
            }
          }}
        >
          Share / save results
        </button>
      )}
      <p>
        Local cache: {((state?.cacheBytes ?? 0) / 1e6).toFixed(1)} MB / 2 GiB.
        Served files stay available for the session. MP3/AAC output is not
        included in this PCM build.
      </p>
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
