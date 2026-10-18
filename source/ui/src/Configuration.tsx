import { resolutions, parseResolution } from "./display";
import { BugReport } from "./BugReport";
import { useRef, useState } from "react";
import { LocalUsb } from "./LocalUsb";
import { CdjConfigurations } from "./CdjConfigurations";
import { TranscodingSettings } from "./Transcoding";
import type { LivePlayer } from "./model";
import type { ReactNode } from "react";
import { ManualLibrary, type DirectPeer } from "./ManualLibrary";
import { LibraryFilterConfiguration } from "./LibraryFilterConfiguration";
import type { Settings } from "./settings";
import { defaults } from "./settings";
import "./studio.css";
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
  const workspace = useRef<HTMLDivElement>(null);
  const [section, setSection] = useState("usb");
  const [bugAvailable, setBugAvailable] = useState(false);
  const sections = [
    {
      id: "connections",
      label: "CDJ connections",
      description: "Connect to the players on your local network.",
      available: !liveEnabled || directPeers.length > 0,
      content: (
        <section className="settings-panel connection-settings">
          <h2>CDJ connections</h2>
          <ManualLibrary
            peers={directPeers}
            liveActive={liveEnabled}
            onConnected={onConnected}
          />
        </section>
      ),
    },
    {
      id: "cdj",
      label: "CDJ configurations",
      description: "Manage saved player profiles and automatic application.",
      available: true,
      content: <CdjConfigurations />,
    },
    {
      id: "usb",
      label: "USB / CDJ status",
      description: "Manage USB libraries and view connected CDJ status.",
      available: true,
      content: (
        <section className="settings-panel">
          <h2>USB / CDJ status</h2>
          <LocalUsb />
        </section>
      ),
    },
    {
      id: "display",
      label: "Display",
      description: "Make the workspace fit your screen and the way you mix.",
      available: true,
      content: (
        <section className="settings-panel display-settings">
          <h2>Display</h2>
          <div className="studio-settings-group">
            <h3>Workspace</h3>
            <label>
              Screen resolution
              <select
                aria-label="Screen resolution"
                value={settings.resolution}
                onChange={(e) =>
                  change("resolution", parseResolution(e.target.value))
                }
              >
                <option value="auto">Automatic — fit current screen</option>
                {Object.entries(resolutions).map(([key, [width, height]]) => (
                  <option key={key} value={key}>
                    {width} × {height}
                  </option>
                ))}
              </select>
            </label>
            <label>
              Extra-wide layout
              <select
                aria-label="Extra-wide layout"
                value={settings.wideLayout}
                onChange={(e) =>
                  change(
                    "wideLayout",
                    e.target.value === "stacked" ? "stacked" : "side-by-side",
                  )
                }
              >
                <option value="side-by-side">Side by side</option>
                <option value="stacked">Stacked</option>
              </select>
            </label>
            <p>
              Side by side applies to extra-wide screens such as 1920 × 720.
            </p>
          </div>
          <div className="studio-settings-group">
            <h3>Waveform comparison</h3>
            <label>
              Waveform comparison
              <select
                aria-label="Waveform comparison"
                value={settings.phaseDisplay}
                onChange={(e) =>
                  change(
                    "phaseDisplay",
                    e.target.value === "waveform" ? "waveform" : "phase",
                  )
                }
              >
                <option value="phase">Beat phase</option>
                <option value="waveform">Reference waveform</option>
              </select>
            </label>
            <ComparisonPreview mode={settings.phaseDisplay} />
            <p>
              Tap the comparison header above either main waveform to switch
              modes.
            </p>
          </div>
          <p>
            Automatic follows the available window or fullscreen size. Presets
            keep their proportions and fit inside the window.
          </p>
          <p>
            Available size: {window.innerWidth} × {window.innerHeight}
          </p>
        </section>
      ),
    },
    {
      id: "waveforms",
      label: "Waveforms",
      description: "Fine-tune waveform rendering and movement.",
      available: true,
      content: (
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
      ),
    },
    {
      id: "track",
      label: "Track information",
      description: "Choose the information shown alongside your decks.",
      available: true,
      content: (
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
      ),
    },
    {
      id: "filters",
      label: "Library filters",
      description: "Organize the filters in your Browse workspace.",
      available: true,
      content: (
        <LibraryFilterConfiguration
          order={settings.libraryFilters}
          bpmRange={settings.libraryBpmRange}
          changeBpmRange={(value) => change("libraryBpmRange", value)}
          categories={libraryCategories}
          change={(value) => change("libraryFilters", value)}
        />
      ),
    },
    {
      id: "audio",
      label: "Audio transcoding",
      description: "Configure audio conversion for compatible playback.",
      available: true,
      content: <TranscodingSettings players={players} />,
    },
    {
      id: "host",
      label: "Host & network",
      description: "Manage this host and its network connections.",
      available: !!desktopTools,
      content: desktopTools,
    },
    {
      id: "reports",
      label: "Bug report",
      description: "Capture and download a report while reproducing an issue.",
      available: bugAvailable,
      content: <BugReport onAvailabilityChange={setBugAvailable} />,
    },
  ];
  const sectionOrder = [
    "usb",
    "host",
    "connections",
    "display",
    "waveforms",
    "track",
    "cdj",
    "filters",
    "audio",
    "reports",
  ];
  sections.sort(
    (a, b) => sectionOrder.indexOf(a.id) - sectionOrder.indexOf(b.id),
  );
  const selected =
    sections.find((item) => item.id === section && item.available) ??
    sections.find((item) => item.id === "display")!;
  return (
    <div className="config-page studio-config">
      <aside className="config-sidebar" aria-label="Settings sections">
        <div className="config-sidebar-heading">
          <h1>Settings</h1>
          <p>Make it yours.</p>
        </div>
        <div className="config-section-links">
          {sections
            .filter((item) => item.available)
            .map((item) => (
              <button
                key={item.id}
                id={`config-link-${item.id}`}
                aria-current={selected.id === item.id ? "page" : undefined}
                aria-controls={`config-panel-${item.id}`}
                onClick={() => {
                  setSection(item.id);
                  if (workspace.current) workspace.current.scrollTop = 0;
                }}
              >
                {item.label}
              </button>
            ))}
        </div>
        <p className="config-sidebar-note">Your workspace, your preferences.</p>
      </aside>
      <div className="config-workspace" ref={workspace}>
        <header className="page-heading">
          <div>
            <small>MENU / PREFERENCES</small>
            <h1>{selected.label}</h1>
            <p>{selected.description}</p>
          </div>
          <button onClick={() => setSettings({ ...defaults })}>
            Restore defaults
          </button>
        </header>
        {sections.map((item) => (
          <div
            key={item.id}
            id={`config-panel-${item.id}`}
            className={`config-panel config-panel-${item.id}`}
            role="region"
            aria-label={item.label}
            hidden={selected.id !== item.id}
          >
            {item.content}
          </div>
        ))}
        <p className="settings-saved" role="status">
          {saveError
            ? "Display preferences apply now, but this browser could not save them."
            : "Display preferences are saved automatically on this device. Save CDJ profiles and connection rules using their Save buttons."}
        </p>
      </div>
    </div>
  );
}

function ComparisonPreview({ mode }: { mode: Settings["phaseDisplay"] }) {
  return (
    <figure className="studio-comparison-preview">
      <figcaption>
        {mode === "phase"
          ? "Beat phase — compare beat alignment"
          : "Reference waveform — see the other deck"}
        <span>Illustrative preview</span>
      </figcaption>
      <svg
        viewBox="0 0 640 96"
        role="img"
        aria-label={
          mode === "phase"
            ? "Orange master beat rail and blue other-deck beat rail with aligned beat markers"
            : "A compact three-band waveform from the other deck above the main waveform"
        }
      >
        <rect width="640" height="96" fill="#252525" />
        {mode === "phase" ? (
          <>
            <text x="12" y="29" fill="#ffac24">
              MASTER
            </text>
            <text x="12" y="73" fill="#29b6ed">
              OTHER DECK
            </text>
            <path d="M100 22H628" stroke="#ffac24" strokeWidth="2" />
            <path d="M100 66H628" stroke="#29b6ed" strokeWidth="2" />
            {Array.from({ length: 12 }, (_, i) => (
              <g key={i}>
                <path
                  d={`M${110 + i * 45} ${i % 4 === 0 ? 16 : 22}V${i % 4 === 0 ? 74 : 68}`}
                  stroke={i % 4 === 0 ? "#f0313b" : "#eee"}
                  strokeWidth={i % 4 === 0 ? 3 : 2}
                />
              </g>
            ))}
          </>
        ) : (
          <>
            <text x="12" y="18" fill="#aaa">
              OTHER DECK · REFERENCE
            </text>
            {Array.from({ length: 100 }, (_, i) => {
              const h =
                6 + Math.abs(Math.sin(i * 1.7) * Math.cos(i * 0.31)) * 39;
              return (
                <g key={i}>
                  <path
                    d={`M${100 + i * 5.2} ${56 - h / 2}v${h}`}
                    stroke="#226ce0"
                    strokeWidth="4"
                  />
                  <path
                    d={`M${100 + i * 5.2} ${56 - h * 0.3}v${h * 0.6}`}
                    stroke="#32c3d2"
                    strokeWidth="4"
                  />
                  <path
                    d={`M${100 + i * 5.2} ${56 - h * 0.12}v${h * 0.24}`}
                    stroke="#e4eafa"
                    strokeWidth="4"
                  />
                </g>
              );
            })}
          </>
        )}
        <path
          d="M270 7v80"
          stroke={mode === "phase" ? "#eee" : "#f0313b"}
          strokeWidth="2"
        />
      </svg>
      <p>
        {mode === "phase"
          ? "The two rails show the master and other deck’s beats. Matching markers indicate aligned beats; red markers indicate the first beat of a bar."
          : "The comparison strip shows the other deck’s waveform above the current deck’s main waveform, so you can compare the two tracks."}
      </p>
    </figure>
  );
}
