import { useEffect, useState } from "react";
import { cueBrowserReports, clearCueBrowserReports } from "./jogTrace";
const preferenceKey = "pc.cue-mode.v2";
const durations = [5, 10, 15, 20, 25, 30];
function preferences() {
  try {
    const p = JSON.parse(localStorage.getItem(preferenceKey) ?? "{}");
    return {
      enabled: typeof p.enabled === "boolean" ? p.enabled : true,
      durationSeconds: durations.includes(p.durationSeconds)
        ? p.durationSeconds
        : 15,
    };
  } catch {
    return { enabled: true, durationSeconds: 15 };
  }
}
type State = {
  enabled: boolean;
  durationSeconds: number;
  active: boolean;
  message: string;
  remainingMs: number | null;
  target: number | null;
  phase: string | null;
  reports: number;
};
export function CueWindow({
  test,
  status,
  onStatus,
}: {
  test: boolean;
  status: boolean;
  onStatus: () => void;
}) {
  const [state, setState] = useState<State | null>(null);
  const [error, setError] = useState("");
  const [prepared, setPrepared] = useState<File | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let cancelled = false,
      pending = false,
      initialized = false;
    // Foreground lease spans tabs inside the app, but never a hidden web page.
    async function poll() {
      if (pending) return;
      pending = true;
      try {
        const response = await fetch(
          "/api/diagnostics/cue-window",
          document.visibilityState === "visible"
            ? {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify(
                  initialized
                    ? { action: "heartbeat" }
                    : { action: "preferences", ...preferences() },
                ),
              }
            : undefined,
        );
        if (response.ok && !cancelled) {
          setState(await response.json());
          if (document.visibilityState === "visible") initialized = true;
        }
      } catch {
        /* Local host may be reconnecting; lease expires independently. */
      } finally {
        pending = false;
      }
    }
    void poll();
    const timer = setInterval(() => void poll(), 250);
    function hidden() {
      if (document.visibilityState === "hidden")
        void fetch("/api/diagnostics/cue-window", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ action: "suspend" }),
          keepalive: true,
        }).catch(() => {});
    }
    document.addEventListener("visibilitychange", hidden);
    return () => {
      cancelled = true;
      clearInterval(timer);
      document.removeEventListener("visibilitychange", hidden);
    };
  }, []);
  async function command(action: string, durationSeconds?: number) {
    setBusy(true);
    setError("");
    try {
      const r = await fetch("/api/diagnostics/cue-window", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ action, durationSeconds }),
      });
      if (!r.ok) throw new Error(await r.text());
      const next: State = await r.json();
      setState(next);
      if (["arm", "disarm", "configure"].includes(action)) {
        localStorage.setItem(
          preferenceKey,
          JSON.stringify({
            enabled: next.enabled,
            durationSeconds: next.durationSeconds,
          }),
        );
      }
      if (action === "arm") onStatus();
      if (action === "clear") {
        clearCueBrowserReports();
        setPrepared(null);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  const message = state?.active
    ? `CDJ${state.target} cue window · ${Math.ceil((state.remainingMs ?? 0) / 1000)}s · ${state.phase}`
    : state?.message;
  if (!test)
    return status && (state?.enabled || state?.active || state?.reports) ? (
      <div className="cue-window-status" role="status">
        {message}
        <button
          onClick={() => void command("disarm")}
          disabled={busy || (!state?.enabled && !state?.active)}
        >
          Disable cue mode
        </button>
      </div>
    ) : null;
  return (
    <section className="settings-panel cue-window-panel">
      <h2>Continuous cue master</h2>
      <p>
        Enabled by default. A new track load or SYNC ON→OFF double tap on the
        paused non-master starts one continuous master hold. The full selected
        duration starts after mastership is confirmed, then the original master
        is restored.
      </p>
      <p>
        Both players must have SYNC OFF and the other player must be playing as
        master. Keep the cueing player outside the main mix. Use CDJ STATUS to
        watch the waveform while cueing.
      </p>
      <p>
        The mode stays enabled until you disable it. Backgrounding, lost status
        or changed playback conditions may end the current hold, but do not turn
        off the mode. If restoration cannot be confirmed, restore the original
        master manually.
      </p>
      <label>
        Master hold duration{" "}
        <select
          aria-label="Master hold duration"
          value={state?.durationSeconds ?? 15}
          disabled={busy || !state}
          onChange={(e) => void command("configure", Number(e.target.value))}
        >
          {durations.map((seconds) => (
            <option key={seconds} value={seconds}>
              {seconds} seconds
            </option>
          ))}
        </select>
      </label>
      <p>
        Duration changes apply to the next window. Your duration and enabled
        setting are saved on this device.
      </p>
      <button
        disabled={busy || !state || state.enabled || state.active}
        onClick={() => void command("arm")}
      >
        Enable and open STATUS
      </button>{" "}
      <button
        disabled={busy || (!state?.enabled && !state?.active)}
        onClick={() => void command("disarm")}
      >
        Disable and restore master
      </button>
      <p role="status">{message || "Disabled"}</p>
      <p>
        Recent recordings: {state?.reports ?? 0}/3. The latest three windows are
        kept; older recordings are replaced without stopping cue mode. Export
        between windows. Recordings are lost on app/host restart.
      </p>
      <button
        disabled={busy || state?.active || !state?.reports}
        onClick={async () => {
          setBusy(true);
          setError("");
          try {
            const r = await fetch("/api/diagnostics/cue-window/reports");
            if (!r.ok) throw new Error(await r.text());
            const host = await r.json();
            setPrepared(
              new File(
                [
                  JSON.stringify(
                    { ...host, browser: cueBrowserReports() },
                    null,
                    2,
                  ),
                ],
                "olc-cue-windows.json",
                { type: "application/json" },
              ),
            );
          } catch (e) {
            setError(String(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        Prepare window reports
      </button>{" "}
      {prepared && (
        <button
          onClick={async () => {
            try {
              if (navigator.canShare?.({ files: [prepared] }))
                await navigator.share({ files: [prepared] });
              else {
                const url = URL.createObjectURL(prepared);
                const a = document.createElement("a");
                a.href = url;
                a.download = prepared.name;
                a.click();
                setTimeout(() => URL.revokeObjectURL(url), 10000);
              }
            } catch (e) {
              setError(String(e));
            }
          }}
        >
          Share / save window reports
        </button>
      )}{" "}
      <button
        disabled={busy || state?.active || !state?.reports}
        onClick={() => void command("clear")}
      >
        Clear recorded windows
      </button>
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
