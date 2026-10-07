import { useEffect, useState } from "react";
type Volume = {
  id: string;
  label: string;
  state: string;
  error?: string | null;
};
type Status = { count: number; volumes: Volume[]; needsFolderAccess: boolean };
export function LocalUsb() {
  const [open, setOpen] = useState(false);
  const [status, setStatus] = useState<Status | null>(null);
  const [error, setError] = useState("");
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
          setError("");
        }
      } catch (e) {
        if (!cancelled) setError(String(e));
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
          <strong>Local USB libraries · {status?.count ?? 0}/3</strong>
          <p>
            OLC checks mounted USBs every 5 seconds. Valid OneLibrary libraries
            appear automatically in the USB selector.
          </p>
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
          {error && <p role="alert">{error}</p>}
        </div>
      )}
    </div>
  );
}
