import { LoadDiagnostics } from "./LoadDiagnostics";
import { OfflinePreview } from "./OfflinePreview";
import type { ReactNode } from "react";
import { JogDiagnostics } from "./JogDiagnostics";
import { CueDiagnostics } from "./CueDiagnostics";

type Props = {
  active: boolean;
  liveEnabled: boolean;
  cueWindow: ReactNode;
  selectLive: () => void;
  zeroSmoothing: () => void;
  previewDeck: number;
  setPreviewDeck: (deck: number) => void;
  busy: boolean;
  load: (file?: File) => Promise<void>;
  error: string;
};

export function TestScreen({
  active,
  liveEnabled,
  cueWindow,
  selectLive,
  zeroSmoothing,
  previewDeck,
  setPreviewDeck,
  busy,
  load,
  error,
}: Props) {
  return (
    <>
      <div className={active ? "config-page test-page" : undefined}>
        <div hidden={!active}>
          <header className="page-heading">
            <div>
              <small>DIAGNOSTICS &amp; EXPERIMENTS</small>
              <h1>TEST</h1>
            </div>
          </header>
          {active && (
            <div className="settings-grid test-tools">
              <CueDiagnostics liveEnabled={liveEnabled} />
              <LoadDiagnostics />
            </div>
          )}
        </div>
        {cueWindow}
        <div hidden={!active}>
          <JogDiagnostics
            active={active}
            selectLive={selectLive}
            zeroSmoothing={zeroSmoothing}
          />
          <div className="settings-grid test-tools">
            <section className="settings-panel">
              <h2>Local USB playback test</h2>
              <p>Test playback of a WAV file served from local USB storage.</p>
              <a
                className="file-button test-tool-link"
                href="/diagnostics/local-usb"
              >
                Open local USB playback test
              </a>
            </section>
            <OfflinePreview
              {...{ previewDeck, setPreviewDeck, busy, load, error }}
            />
          </div>
        </div>
      </div>
    </>
  );
}
