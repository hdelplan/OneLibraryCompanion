import { useEffect, useState } from "react";
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
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try {
        const next = await api<ConversionState>(
          "/api/transcoding",
          undefined,
          controller.signal,
        );
        if (controller.signal.aborted) return;
        setState(next);
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
      <p>
        Local cache: {((state?.cacheBytes ?? 0) / 1e6).toFixed(1)} MB / 2 GiB.
        Served files stay available for the session. MP3/AAC output is not
        included in this PCM build.
      </p>
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
