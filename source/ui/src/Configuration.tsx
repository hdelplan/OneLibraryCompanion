import { TranscodingSettings } from "./Transcoding";
import type { LivePlayer } from "./model";
import type { ReactNode } from "react";
import { ManualLibrary, type DirectPeer } from "./ManualLibrary";
import { LibraryFilterConfiguration } from "./LibraryFilterConfiguration";
import type { Settings } from "./settings";
import { defaults } from "./settings";
type Props = {
  players: LivePlayer[];
  desktopTools?: ReactNode;
  directPeers: DirectPeer[];
  onConnected: (source?: string) => void;
  settings: Settings;
  libraryCategories: string[];
  setSettings: (settings: Settings) => void;
  change: <K extends keyof Settings>(key: K, value: Settings[K]) => void;
  saveError: boolean;
  liveEnabled: boolean;
  showLive: boolean;
  selectLive: () => void;
};
export function Configuration({
  players,
  desktopTools,
  directPeers,
  onConnected,
  settings,
  libraryCategories,
  setSettings,
  change,
  saveError,
  liveEnabled,
  showLive,
  selectLive,
}: Props) {
  return (
    <div className="config-page">
      <header className="page-heading">
        <div>
          <small>CONNECTIONS &amp; PREFERENCES</small>
          <h1>MENU</h1>
        </div>
        <button onClick={() => setSettings({ ...defaults })}>
          Restore defaults
        </button>
      </header>
      {(!liveEnabled || directPeers.length > 0) && (
        <section className="settings-panel connection-settings">
          <h2>CDJ connections</h2>
          <ManualLibrary
            peers={directPeers}
            liveActive={liveEnabled}
            onConnected={onConnected}
          />
        </section>
      )}
      <div className="settings-grid">
        {desktopTools}
        <TranscodingSettings players={players} />
        <section className="settings-panel">
          <h2>Waveforms</h2>
          <label>
            Jog smoothing{" "}
            <select
              value={settings.jogSmoothing}
              onChange={(e) => change("jogSmoothing", Number(e.target.value))}
            >
              <option value={0}>Off — immediate</option>
              <option value={20}>Responsive — 20 ms</option>
              <option value={50}>Balanced — 50 ms</option>
              <option value={100}>Smooth — 100 ms</option>
            </select>
          </label>

          <label>
            Playhead position
            <select
              value={settings.playhead}
              onChange={(e) =>
                change("playhead", e.target.value as Settings["playhead"])
              }
            >
              <option value="third">One third from the left</option>
              <option value="center">Center</option>
            </select>
          </label>
          <label>
            Playhead color
            <select
              value={settings.playheadColor}
              onChange={(e) =>
                change(
                  "playheadColor",
                  e.target.value as Settings["playheadColor"],
                )
              }
            >
              <option value="red">Red</option>
              <option value="white">White</option>
            </select>
          </label>
          <label>
            Visible time window
            <select
              value={settings.window}
              onChange={(e) => change("window", +e.target.value)}
            >
              {Array.from({ length: 16 }, (_, i) => 2 + i * 2).map((n) => (
                <option key={n} value={n}>
                  {n} seconds
                </option>
              ))}
            </select>
          </label>
          <label>
            Three-band rendering
            <select
              value={settings.bass}
              onChange={(e) =>
                change("bass", e.target.value as Settings["bass"])
              }
            >
              <option value="emphasis">Layered · slight bass lift</option>
              <option value="balanced">Layered band balance</option>
            </select>
          </label>
          <p>
            Square-root band envelopes with cached geometry. Optional bass lift
            brings out quieter low-frequency detail.
          </p>
        </section>
        <section className="settings-panel">
          <h2>Track information</h2>
          <label>
            Primary time display
            <select
              value={settings.time}
              onChange={(e) =>
                change("time", e.target.value as Settings["time"])
              }
            >
              <option value="remaining">Time remaining</option>
              <option value="elapsed">Elapsed time</option>
            </select>
          </label>
          <label className="toggle">
            <span>Show overview waveforms</span>
            <input
              type="checkbox"
              checked={settings.overview}
              onChange={(e) => change("overview", e.target.checked)}
            />
          </label>
          <label className="toggle">
            <span>Show additional track details</span>
            <input
              type="checkbox"
              checked={settings.details}
              onChange={(e) => change("details", e.target.checked)}
            />
          </label>
          <label>
            Phrase analysis
            <input
              type="checkbox"
              checked={settings.phrases}
              onChange={(e) => change("phrases", e.target.checked)}
            />
          </label>
          <p>
            Title, artist, key, BPM, pitch and time remain on the main display.
            Missing values appear as dashes.
          </p>
          <div className="connection-note">
            <span className="status-dot" />{" "}
            {liveEnabled
              ? "LIVE MONITOR AVAILABLE"
              : "LIVE CONNECTION NOT ACTIVE"}
            <p>
              Load tracks from BROWSE; control playback on the CDJs.{" "}
              {showLive
                ? "Displaying live status."
                : "Offline preview selected."}
            </p>
            {liveEnabled && (
              <button onClick={selectLive}>Show live CDJs</button>
            )}
          </div>
        </section>
        <LibraryFilterConfiguration
          order={settings.libraryFilters}
          bpmRange={settings.libraryBpmRange}
          changeBpmRange={(value) => change("libraryBpmRange", value)}
          categories={libraryCategories}
          change={(value) => change("libraryFilters", value)}
        />
      </div>
      <p className="settings-saved">
        {saveError
          ? "Settings apply now, but this browser could not save them."
          : "Settings are saved automatically on this device."}
      </p>
    </div>
  );
}
