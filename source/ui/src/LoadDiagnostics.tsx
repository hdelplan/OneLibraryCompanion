import { useState } from "react";

export function LoadDiagnostics() {
  const [error, setError] = useState("");
  const [report, setReport] = useState<File | null>(null);
  const [preparing, setPreparing] = useState(false);
  const bridge = (
    window as unknown as {
      webkit?: {
        messageHandlers?: { localUsb?: { postMessage(value: unknown): void } };
      };
    }
  ).webkit?.messageHandlers?.localUsb;
  return (
    <section className="settings-panel">
      <h2>Local USB load diagnostics</h2>
      <p>
        Capture discovery and file-serving activity to investigate failed CDJ
        loads.
      </p>
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
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
