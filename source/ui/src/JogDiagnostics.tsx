import { useEffect, useState } from "react";
import { syncJogTrace, stopJogTrace } from "./jogTrace";
import {
  handoffPrompt,
  syncTapPrompt,
  type CaptureState,
} from "./handoffCapture";
export function JogDiagnostics({
  active,
  selectLive,
  zeroSmoothing,
}: {
  active: boolean;
  selectLive: () => void;
  zeroSmoothing: () => void;
}) {
  const [message, setMessage] = useState("");
  const [prepared, setPrepared] = useState<File | null>(null);
  const [busy, setBusy] = useState(false);
  const [captureState, setCaptureState] = useState<CaptureState | null>(null);
  const capturing = captureState?.active === true;
  const bridgeRemaining =
    capturing && captureState?.profile === "bridge-c0"
      ? captureState.remainingMs
      : null;
  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    const poll = async () => {
      try {
        const response = await fetch("/api/diagnostics/jog/state");
        if (!response.ok) return;
        const state = await response.json();
        if (!cancelled) setCaptureState(state);
      } catch {
        /* Next poll retries while the local host reconnects. */
      }
    };
    void poll();
    const timer = setInterval(() => {
      void poll();
    }, 500);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [active]);
  async function request(
    start: boolean,
    bridge = false,
    handoff = false,
    syncTap = false,
  ) {
    const response = await fetch("/api/diagnostics/jog", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ start, bridge, handoff, syncTap }),
    });
    if (!response.ok) throw new Error(await response.text());
    return await response.json();
  }
  return (
    <section className="settings-panel">
      <h2>Jog response capture</h2>
      <p>
        Capture up to 45 seconds while STATUS is visible: pause and jog slowly,
        reverse direction, then play and nudge. Return to TEST to download the
        report. Keep this app in the foreground.
      </p>
      <p>
        Forward playback and detected whole-beat loops use each deck’s beat
        packets and motion speed. This capture records the signals and displayed
        positions for analysis.
      </p>
      <button onClick={zeroSmoothing}>Set jog smoothing to 0 ms</button>{" "}
      <button
        disabled={busy || capturing}
        onClick={async () => {
          setBusy(true);
          try {
            const state = await request(true);
            syncJogTrace(state);
            setCaptureState(state);
            setPrepared(null);
            setMessage(
              "Capture started. Return to TEST to download after testing.",
            );
            selectLive();
          } catch (e) {
            setMessage(String(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        Start capture and open STATUS
      </button>{" "}
      <button
        disabled={busy}
        onClick={async () => {
          setBusy(true);
          try {
            const state = await request(false);
            setCaptureState(state);
            const response = await fetch("/api/diagnostics/jog");
            if (!response.ok) throw new Error(await response.text());
            const report = {
              host: await response.json(),
              browser: stopJogTrace(),
            };
            setPrepared(
              new File(
                [JSON.stringify(report, null, 2)],
                report.host.experiment?.profile === "sync-double-tap"
                  ? "olc-sync-double-tap.json"
                  : report.host.experiment?.profile === "manual-master-handoff"
                    ? "olc-master-handoff.json"
                    : "olc-jog-response.json",
                { type: "application/json" },
              ),
            );
            setMessage(
              report.host.experiment?.profile === "sync-double-tap"
                ? `Sync report ready (${report.host.experiment.outcome}). Ensure CDJ2 Sync is OFF. Tap Share / save report.`
                : report.host.experiment?.profile === "manual-master-handoff"
                  ? `Handoff report ready (${report.host.experiment.outcome}). Confirm CDJ1 MASTER is restored. Tap Share / save report.`
                  : report.host.experiment?.profile === "bridge-c0"
                    ? "Bridge report ready. Live waveform frames are intentionally absent. Tap Share / save report."
                    : report.browser.frames.length
                      ? `Report ready: ${report.browser.frames.length} frames. Tap Share / save report.`
                      : `Capture incomplete: ${report.browser.warnings.join(" ")} Save it for diagnosis.`,
            );
          } catch (e) {
            setMessage(String(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        Stop and prepare report
      </button>
      <h3>Sync double-tap capture</h3>
      <p>
        Start with Sync OFF on both CDJs. Keep CDJ1 playing as master and CDJ2
        loaded, paused and out of the main mix. This separate test records the
        physical CDJ2 SYNC button. Mastership stays on CDJ1.
      </p>
      <p>
        Follow the 45-second guide: one slow pair of presses, then three quick
        double-tap attempts. Wait between attempts and finish with CDJ2 Sync
        OFF. Do not touch MASTER, Play or the jog wheel. This test never starts
        the proposed 20-second master-switching mode. A video of the button and
        its light can help identify presses that the network did not report.
      </p>
      <button
        disabled={busy || capturing}
        onClick={async () => {
          setBusy(true);
          try {
            const state = await request(true, false, false, true);
            syncJogTrace(state);
            setCaptureState(state);
            setPrepared(null);
            setMessage(
              "Sync double-tap capture started. Follow the guide below.",
            );
          } catch (e) {
            setMessage(String(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        Start Sync double-tap capture
      </button>
      {captureState?.profile === "sync-double-tap" && (
        <>
          <p role="status">{syncTapPrompt(captureState)}</p>
          {captureState.syncTap && (
            <p>
              Reported CDJ2 Sync:{" "}
              {captureState.syncTap.sync === null
                ? "unknown"
                : captureState.syncTap.sync
                  ? "ON"
                  : "OFF"}
              .
              {` Observed flag changes: ${captureState.syncTap.edges}; ON/OFF pulses: ${captureState.syncTap.pulses}; short pulse candidates: ${captureState.syncTap.shortPulseCandidates}.`}
              {" These counts do not prove individual button presses."}
            </p>
          )}
        </>
      )}
      <h3>Manual master handoff capture</h3>
      <p>
        Turn Sync OFF on both CDJs. Keep CDJ1 playing as master and CDJ2 out of
        the main mix. On CDJ2, load an analyzed track, play briefly, then pause
        with PLAY/PAUSE away from the track start. Set jog smoothing to 0 ms.
      </p>
      <p>
        Stay here and follow the 45-second guide. Jog CDJ2 with short forward
        and backward movements throughout. At 12 seconds press CDJ2 MASTER; at
        25 seconds press CDJ1 MASTER. Normal status capture continues. You
        change master using the physical buttons; this test sends no master
        commands.
      </p>
      <button
        disabled={busy || capturing}
        onClick={async () => {
          setBusy(true);
          try {
            const state = await request(true, false, true);
            syncJogTrace(state);
            setCaptureState(state);
            setPrepared(null);
            setMessage("Handoff capture started. Follow the guide below.");
          } catch (e) {
            setMessage(String(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        Start manual handoff capture
      </button>
      {captureState?.profile === "manual-master-handoff" && (
        <p role="status">{handoffPrompt(captureState)}</p>
      )}
      <p>
        Experimental bridge telemetry: keep CDJ1 playing as master and pause
        CDJ2. Start the test, wait 5 seconds, then jog CDJ2 forward and backward
        with Sync on for 15 seconds and off for 15 seconds. Keep this app in the
        foreground. Live waveforms pause during this 45-second test; the normal
        connection resumes automatically. Unverified on the original nexus.
      </p>
      <button
        disabled={busy || capturing}
        onClick={async () => {
          setBusy(true);
          try {
            const state = await request(true, true);
            syncJogTrace(state);
            setCaptureState(state);
            setPrepared(null);
            setMessage(
              "Bridge capture started. Wait 5 seconds, then jog CDJ2. After 45 seconds, stop and prepare the report.",
            );
          } catch (e) {
            setMessage(String(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        Start bridge telemetry test
      </button>
      {bridgeRemaining !== null && (
        <p role="status">
          {bridgeRemaining > 40000
            ? `Wait ${Math.ceil((bridgeRemaining - 40000) / 1000)} seconds before jogging.`
            : bridgeRemaining > 25000
              ? "Jog CDJ2 forward and backward with Sync ON."
              : bridgeRemaining > 10000
                ? "Turn CDJ2 Sync OFF and continue jogging."
                : "Hold CDJ2 still for the rest of the capture."}
          {` ${Math.ceil(bridgeRemaining / 1000)} seconds remaining.`}
        </p>
      )}
      {prepared && (
        <button
          onClick={async () => {
            try {
              if (navigator.canShare?.({ files: [prepared] })) {
                await navigator.share({
                  files: [prepared],
                  title: "OLC jog response",
                });
              } else {
                const url = URL.createObjectURL(prepared);
                const a = document.createElement("a");
                a.href = url;
                a.download = prepared.name;
                a.click();
                setTimeout(() => URL.revokeObjectURL(url), 60000);
              }
            } catch (e) {
              setMessage(String(e));
            }
          }}
        >
          Share / save report
        </button>
      )}
      {message && <p role="status">{message}</p>}
    </section>
  );
}
