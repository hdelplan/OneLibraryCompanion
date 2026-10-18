import { useEffect, useState } from "react";
import { syncJogTrace, stopJogTrace } from "./jogTrace";
import { renderPerformanceReport } from "./renderPerformance";

let pending: Promise<void> | null = null;
let captureUntil = 0;
let reportSession = "";
let message = "";
let errors: object[] = [];
export function recordBugError(detail: string) {
  if (performance.now() <= captureUntil && errors.length < 100)
    errors.push({
      at: new Date().toISOString(),
      detail: detail.slice(0, 2000),
    });
}
if (typeof window !== "undefined") {
  window.addEventListener("error", (event) => recordBugError(event.message));
  window.addEventListener("unhandledrejection", (event) =>
    recordBugError(String(event.reason)),
  );
}
async function request(path: string, body?: unknown) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 10000);
  try {
    const response = await fetch(path, {
      signal: controller.signal,
      ...(body === undefined
        ? {}
        : {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(body),
          }),
    });
    if (!response.ok) throw new Error(await response.text());
    return await response.json();
  } finally {
    clearTimeout(timer);
  }
}
async function capture() {
  const state = await request("/api/bug-report", {});
  errors = [];
  reportSession = state.session;
  captureUntil = performance.now() + 60000;
  syncJogTrace(state);
  message =
    "Recording for one minute. Return to STATUS or BROWSE and reproduce the bug.";
  // Keep this task alive when MENU unmounts. Retry upload if the host reconnects.
  await new Promise((resolve) => setTimeout(resolve, 61000));
  const browser = {
    ...stopJogTrace(),
    errors,
    userAgent: navigator.userAgent,
    waveformPerformance: JSON.parse(renderPerformanceReport()),
  };
  for (let attempt = 0; attempt < 10; attempt++) {
    try {
      await request("/api/bug-report/browser", {
        session: state.session,
        browser,
      });
      message = "Bug report saved on the RPi. You can download a copy below.";
      return;
    } catch (error) {
      message = `Report upload failed: ${String(error)}. Retrying…`;
      await new Promise((resolve) => setTimeout(resolve, 2000));
    }
  }
  message =
    "Browser report could not be saved on the RPi. Download the local copy before reloading.";
  localReport = { session: state.session, browser };
}
let localReport: unknown = null;
export function BugReport({
  onAvailabilityChange,
}: {
  onAvailabilityChange?: (enabled: boolean) => void;
}) {
  const [enabled, setEnabled] = useState(false);
  const [status, setStatus] = useState(message);
  const [busy, setBusy] = useState(!!pending);
  const [available, setAvailable] = useState(false);
  useEffect(() => {
    let stopped = false;
    async function poll() {
      setBusy(!!pending);
      setStatus(message);
      if (localReport) setAvailable(true);
      try {
        const state = await request("/api/bug-report");
        if (stopped) return;
        setEnabled(state.enabled === true);
        setBusy(!!pending || state.capture.active === true);
        setAvailable(!!state.savedSession || !!localReport);
        setStatus(
          state.saveError
            ? `Could not save host report: ${state.saveError}`
            : pending && captureUntil > performance.now()
              ? `Recording bug report · ${Math.ceil((captureUntil - performance.now()) / 1000)} seconds left. Return to STATUS or BROWSE.`
              : message ||
                (state.savedSession ? "Bug report saved on the RPi." : ""),
        );
      } catch {
        /* Capability remains hidden if host is unavailable. */
      }
    }
    void poll();
    const timer = setInterval(() => void poll(), 1000);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, []);
  useEffect(() => {
    onAvailabilityChange?.(enabled);
  }, [enabled, onAvailabilityChange]);
  if (!enabled) return null;
  return (
    <section className="settings-panel">
      <h2>Bug report</h2>
      <p>
        Record one minute of CDJ timing, waveform performance and browser
        errors. Keep this browser open, then reproduce the issue. Reports are
        saved on this RPi.
      </p>
      <button
        disabled={busy}
        onClick={() => {
          setBusy(true);
          localReport = null;
          pending = capture()
            .catch((error) => {
              message = String(error);
            })
            .finally(() => {
              pending = null;
              captureUntil = 0;
            });
        }}
      >
        Bug report
      </button>
      {status && <p role="status">{status}</p>}
      {available && (
        <button
          onClick={async () => {
            try {
              const report =
                localReport ?? (await request("/api/bug-report/latest"));
              const url = URL.createObjectURL(
                new Blob([JSON.stringify(report)], {
                  type: "application/json",
                }),
              );
              const link = document.createElement("a");
              link.href = url;
              link.download = `olc-bug-report-${reportSession || "latest"}.json`;
              link.click();
              setTimeout(() => URL.revokeObjectURL(url), 1000);
            } catch (error) {
              setStatus(String(error));
            }
          }}
        >
          Download bug report
        </button>
      )}
    </section>
  );
}
