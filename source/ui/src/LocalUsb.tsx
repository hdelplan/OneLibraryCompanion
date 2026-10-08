import { useEffect, useState } from "react";
type Volume = {
  id: string;
  label: string;
  state: string;
  error?: string | null;
};
type Status = {
  count: number;
  volumes: Volume[];
  needsFolderAccess: boolean;
  serving?: {
    active?: boolean;
    source?: string | null;
    error?: string;
    peers?: { ip: string; announced: boolean; mediaQueried: boolean }[];
  };
};
export function LocalUsb() {
  const [open, setOpen] = useState(false);
  const [status, setStatus] = useState<Status | null>(null);
  const [error, setError] = useState("");
  const [report, setReport] = useState<File | null>(null);
  const [preparing, setPreparing] = useState(false);
  const [pollError, setPollError] = useState("");
  const bridge = (
    window as unknown as {
      webkit?: {
        messageHandlers?: { localUsb?: { postMessage(value: unknown): void } };
      };
    }
  ).webkit?.messageHandlers?.localUsb;
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try {
        const response = await fetch("/api/library/local");
        if (!response.ok) throw new Error(await response.text());
        const result = (await response.json()) as Status | null;
        if (!cancelled) {
          setStatus(result);
          setPollError("");
        }
      } catch (e) {
        if (!cancelled) setPollError(String(e));
      }
      if (!cancelled) timer = setTimeout(() => void poll(), 1500);
    }
    void poll();
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [open]);
  return (
    <div className="local-usb-control">
      <button onClick={() => setOpen(!open)} aria-expanded={open}>
        LOCAL USB
      </button>
      {open && (
        <div
          className="local-usb-panel"
          role="dialog"
          aria-label="Local USB libraries"
        >
          <button onClick={() => setOpen(false)} aria-label="Close local USB">
            Close
          </button>
          <strong>Mounted USB libraries: {status?.count ?? 0}</strong>
          <p>
            OLC checks mounted USBs every 5 seconds. Valid OneLibrary libraries
            appear automatically in the USB selector.
          </p>
          <p>
            OLC serves one local USB library per connection session. The first
            track loaded selects that library. To switch libraries, stop both
            CDJs, disconnect both in OLC, then reconnect. Compatible tracks use
            their original USB track IDs, paths and audio.
          </p>
          <p>
            {status?.serving?.active
              ? "Local source running; tracks are prepared when selected."
              : "Local source waiting for USB and a live Direct IP connection."}
          </p>
          {status?.serving?.source && (
            <p>
              Serving USB:{" "}
              {status.volumes.find(
                (v) => `local-usb:${v.id}` === status.serving?.source,
              )?.label ?? "Selected library"}
            </p>
          )}
          <strong>Connected CDJs</strong>
          {status?.serving?.peers?.map((peer) => (
            <p key={peer.ip}>
              {peer.ip}:{" "}
              {peer.mediaQueried && peer.announced
                ? "Connected to OLC’s local source"
                : "Discovering OLC’s local source…"}
            </p>
          ))}
          {status?.serving?.error && <p role="alert">{status.serving.error}</p>}
          <strong>Load diagnostics</strong>
          <button
            disabled={preparing}
            onClick={() => {
              setPreparing(true);
              setError("");
              setReport(null);
              void fetch("/api/library/local/trace")
                .then((r) => {
                  if (!r.ok) throw new Error("Cannot read load diagnostics");
                  return r.json();
                })
                .then((value) => {
                  setReport(
                    new File(
                      [JSON.stringify(value, null, 2)],
                      "olc-local-load-trace.json",
                      { type: "application/json" },
                    ),
                  );
                })
                .catch((e) => setError(String(e)))
                .finally(() => setPreparing(false));
            }}
          >
            {preparing ? "Preparing diagnostics…" : "Prepare load diagnostics"}
          </button>
          {report && (
            <button
              onClick={async () => {
                try {
                  setError("");
                  if (bridge) {
                    bridge.postMessage({
                      shareDiagnostics: await report.text(),
                    });
                  } else if (navigator.canShare?.({ files: [report] })) {
                    await navigator.share({
                      files: [report],
                      title: "OLC local load diagnostics",
                    });
                  } else {
                    const url = URL.createObjectURL(report);
                    const link = document.createElement("a");
                    link.href = url;
                    link.download = report.name;
                    link.click();
                    setTimeout(() => URL.revokeObjectURL(url), 60000);
                  }
                } catch (e) {
                  setError(String(e));
                }
              }}
            >
              Share / save diagnostics
            </button>
          )}
          {bridge && (
            <>
              <p>
                Choose each USB's root folder once in Files to allow access. OLC
                remembers up to three folders.
              </p>
              <button onClick={() => bridge.postMessage("choose")}>
                Choose USB folder
              </button>
              <button onClick={() => bridge.postMessage("forgetAll")}>
                Forget saved folders
              </button>
            </>
          )}
          {status?.volumes.map((v) => (
            <div key={v.id} className="local-usb-volume">
              <b>{v.label}</b>
              <span>{v.state}</span>
              {v.error && <p role="alert">{v.error}</p>}
              {bridge && (
                <button onClick={() => bridge.postMessage({ forget: v.id })}>
                  Forget
                </button>
              )}
            </div>
          ))}
          {!status?.volumes.length && <p>No local USB library detected.</p>}
          {pollError && <p role="alert">{pollError}</p>}
          {error && <p role="alert">{error}</p>}
        </div>
      )}
    </div>
  );
}
