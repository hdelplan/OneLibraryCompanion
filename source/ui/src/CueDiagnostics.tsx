import { useEffect, useState } from "react";
type Report = {
  active?: boolean;
  phase?: string;
  error?: string;
  track?: { title?: string };
  nativeReply?: unknown;
};
function savedIp(number: number) {
  try {
    return localStorage.getItem(`pc.cdj-ip.${number}`) ?? "";
  } catch {
    return "";
  }
}
export function CueDiagnostics({ liveEnabled }: { liveEnabled: boolean }) {
  const [number, setNumber] = useState(2);
  const [ip, setIp] = useState(() => savedIp(2));
  const [report, setReport] = useState<Report | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try {
        const response = await fetch("/api/diagnostics/native-cues");
        if (!response.ok) throw new Error(await response.text());
        const value = (await response.json()) as Report;
        if (!cancelled) setReport(value);
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
      if (!cancelled) timer = setTimeout(() => void poll(), 1000);
    }
    void poll();
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, []);
  async function capture(stop = false) {
    setBusy(true);
    setError("");
    try {
      const response = await fetch("/api/diagnostics/native-cues", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ ip: ip.trim(), number, stop }),
      });
      if (!response.ok) throw new Error(await response.text());
      setReport((await response.json()) as Report);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  async function share() {
    try {
      setError("");
      const text = JSON.stringify(report, null, 2);
      const file = new File([text], `olc-native-cues-cdj${number}.json`, {
        type: "application/json",
      });
      const bridge = (
        window as unknown as {
          webkit?: {
            messageHandlers?: {
              localUsb?: { postMessage(value: unknown): void };
            };
          };
        }
      ).webkit?.messageHandlers?.localUsb;
      if (bridge)
        bridge.postMessage({
          shareDiagnostics: text,
          diagnosticKind: "native-cues",
        });
      else if (navigator.canShare?.({ files: [file] }))
        await navigator.share({
          files: [file],
          title: "OLC native cue diagnostics",
        });
      else {
        const url = URL.createObjectURL(file);
        const link = document.createElement("a");
        link.href = url;
        link.download = file.name;
        link.click();
        setTimeout(() => URL.revokeObjectURL(url), 60000);
      }
    } catch (e) {
      setError(String(e));
    }
  }
  return (
    <section className="settings-panel">
      <h2>CDJ cue diagnostics</h2>
      <p>
        Plug the USB into the CDJ and load the track directly on that player.
        Disconnect both OLC connections in MENU, then capture its cue data here.
        Playback and USB files are not changed.
      </p>
      <p>
        Player number 4 must be unused. The capture takes up to 90 seconds. Keep
        the same track loaded until it finishes.
      </p>
      <label>
        Player{" "}
        <select
          value={number}
          disabled={busy || report?.active}
          onChange={(e) => {
            const n = Number(e.target.value);
            setNumber(n);
            setIp(savedIp(n));
          }}
        >
          <option value={1}>CDJ 1</option>
          <option value={2}>CDJ 2</option>
        </select>
      </label>
      <label>
        CDJ IP address{" "}
        <input
          value={ip}
          disabled={busy || report?.active}
          onChange={(e) => setIp(e.target.value)}
          placeholder="192.168.10.…"
        />
      </label>
      <button
        disabled={busy || report?.active || liveEnabled || !ip.trim()}
        onClick={() => void capture()}
      >
        Capture native cues
      </button>
      {liveEnabled && (
        <p>Disconnect OLC’s CDJ connections in MENU before capturing.</p>
      )}
      {report?.active && (
        <button disabled={busy} onClick={() => void capture(true)}>
          Stop capture
        </button>
      )}
      <p role="status">
        {report?.phase ?? "Ready"}
        {report?.track?.title ? ` · ${report.track.title}` : ""}
      </p>
      {report && !report.active && report.phase !== "Ready" && (
        <button onClick={() => void share()}>Share / save cue report</button>
      )}
      {report?.error && <p role="alert">{report.error}</p>}
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
