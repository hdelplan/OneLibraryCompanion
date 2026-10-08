import { lazy, Suspense, useCallback, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import type { Analysis, Deck } from "./model";
import { durationOf } from "./model";
import { Settings, readSettings, settingsKey, timeLabel } from "./settings";
import { loadAnalysis } from "./api";
import { Library } from "./Library";
import { activeMixKey } from "./keyCompatibility";
import { SetHistory } from "./SetHistory";
import { Configuration } from "./Configuration";
import { OfflinePreview } from "./OfflinePreview";
import { HostSettings } from "./HostSettings";
const Experiments = __OLC_EXPERIMENTS__
  ? lazy(() => import("./Experiments"))
  : null;
import { StatusDisplay } from "./StatusDisplay";
import "./style.css";
import { useLiveDecks } from "./useLiveDecks";
function App() {
  const [libraryToolbar, setLibraryToolbar] = useState<HTMLDivElement | null>(
    null,
  );
  const live = useLiveDecks();
  const [libraryCategories, setLibraryCategories] = useState<string[]>([]);
  const [offlinePreview, setOfflinePreview] = useState(false);
  useEffect(() => {
    if (live.enabled) setOfflinePreview(false);
  }, [live.enabled]);
  const [settings, setSettings] = useState<Settings>(readSettings),
    [page, setPage] = useState<
      "status" | "library" | "history" | "config" | "test"
    >(() =>
      __OLC_EXPERIMENTS__ && location.hash === "#test"
        ? "test"
        : location.hash === "#set-history"
          ? "history"
          : "status",
    );
  const [librarySourceRequest, setLibrarySourceRequest] = useState<{
    id: string;
  } | null>(null);
  const [scale, setScale] = useState(1),
    [decks, setDecks] = useState<[Deck, Deck]>([null, null]);
  const [previewDeck, setPreviewDeck] = useState(0),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [saveError, setSaveError] = useState(false);
  useEffect(() => {
    const resize = () =>
      setScale(Math.min(innerWidth / 1280, innerHeight / 800));
    resize();
    addEventListener("resize", resize);
    return () => removeEventListener("resize", resize);
  }, []);
  useEffect(() => {
    try {
      localStorage.setItem(settingsKey, JSON.stringify(settings));
      setSaveError(false);
    } catch {
      setSaveError(true);
    }
  }, [settings]);
  function change<K extends keyof Settings>(key: K, value: Settings[K]) {
    setSettings((s) => ({ ...s, [key]: value }));
  }
  async function load(file?: File) {
    setBusy(true);
    setError("");
    try {
      const analysis = await loadAnalysis(file);
      setOfflinePreview(true);
      setDecks((prev) => {
        const next: [Deck, Deck] = [...prev];
        next[previewDeck] = {
          name: file?.name ?? "Saved CDJ capture",
          source: "offline",
          analysis,
          position: 0,
        };
        return next;
      });
      setPage("status");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  const previewLibrary = useCallback(
    (analysis: Analysis) => {
      setOfflinePreview(true);
      setDecks((previous) => {
        const next: [Deck, Deck] = [...previous];
        next[previewDeck] = {
          name: analysis.track?.title ?? "USB waveform preview",
          source: "offline",
          analysis,
          position: 0,
        };
        return next;
      });
      setPage("status");
    },
    [previewDeck],
  );
  const showLoadedTrack = useCallback(() => {
    setOfflinePreview(false);
    setPage("status");
  }, []);
  function scrub(value: number) {
    setDecks((prev) => {
      const next: [Deck, Deck] = [...prev];
      const d = next[previewDeck];
      if (d) next[previewDeck] = { ...d, position: value };
      return next;
    });
  }
  const preview = decks[previewDeck],
    previewDuration = durationOf(preview) ?? 0;
  const showLive = live.enabled && !offlinePreview;
  const shownDecks = showLive ? live.decks : decks;

  return (
    <main style={{ width: 1280 * scale, height: 800 * scale }}>
      <div className="screen" style={{ transform: `scale(${scale})` }}>
        <nav className={page === "library" ? "browse-nav" : undefined}>
          <span className="brand" title="OneLibraryCompanion">
            OneLibraryCompanion
          </span>
          <span className="connection">
            <i className="status-dot" />{" "}
            {showLive
              ? live.error
                ? "CONNECTION ERROR"
                : "LIVE MONITOR"
              : decks.some(Boolean)
                ? "SAVED CAPTURE"
                : "NO LIVE CONNECTION"}
          </span>
          <div
            ref={setLibraryToolbar}
            className="library-toolbar-slot"
            hidden={page !== "library"}
          />
          {page === "status" && (
            <div
              className="waveform-zoom"
              role="group"
              aria-label="Zoom both waveforms"
            >
              <button
                aria-label="Zoom out both waveforms"
                disabled={settings.window >= 32}
                onClick={() => change("window", settings.window + 2)}
              >
                −
              </button>
              <span>{settings.window}s</span>
              <button
                aria-label="Zoom in both waveforms"
                disabled={settings.window <= 2}
                onClick={() => change("window", settings.window - 2)}
              >
                +
              </button>
            </div>
          )}
          <div className="navigation">
            <button
              aria-current={page === "status" ? "page" : undefined}
              onClick={() => setPage("status")}
            >
              CDJ STATUS
            </button>
            <button
              aria-current={page === "library" ? "page" : undefined}
              onClick={() => setPage("library")}
            >
              BROWSE
            </button>
            <button
              aria-current={page === "history" ? "page" : undefined}
              onClick={() => setPage("history")}
            >
              SET HISTORY
            </button>
            <button
              aria-current={page === "config" ? "page" : undefined}
              onClick={() => setPage("config")}
            >
              MENU
            </button>
            {__OLC_EXPERIMENTS__ && (
              <button
                aria-current={page === "test" ? "page" : undefined}
                onClick={() => setPage("test")}
              >
                TEST
              </button>
            )}
          </div>
        </nav>
        <Library
          toolbarTarget={libraryToolbar}
          activeKey={activeMixKey(live.decks)}
          directAllowed={!live.enabled}
          sourceRequest={librarySourceRequest}
          onPreview={previewLibrary}
          onLoaded={showLoadedTrack}
          active={page === "library"}
          filterOrder={settings.libraryFilters}
          bpmRange={settings.libraryBpmRange}
          onCategories={setLibraryCategories}
          players={live.decks.flatMap((deck) =>
            deck?.live ? [deck.live] : [],
          )}
        />
        <SetHistory active={page === "history"} />
        {Experiments && (
          <Suspense fallback={null}>
            <Experiments
              page={page}
              selectLive={showLoadedTrack}
              zeroSmoothing={() => change("jogSmoothing", 0)}
              {...{ previewDeck, setPreviewDeck, busy, load, error }}
            />
          </Suspense>
        )}

        {page === "library" ||
        page === "history" ||
        page === "test" ? null : page === "config" ? (
          <Configuration
            players={live.decks.flatMap((deck) =>
              deck?.live ? [deck.live] : [],
            )}
            desktopTools={
              !__OLC_EXPERIMENTS__ ? (
                <>
                  <HostSettings />
                  <OfflinePreview
                    {...{ previewDeck, setPreviewDeck, busy, load, error }}
                  />
                </>
              ) : null
            }
            directPeers={live.directPeers}
            onConnected={(id) => {
              if (id) {
                setLibrarySourceRequest({ id });
                setPage("library");
              }
            }}
            {...{
              settings,
              libraryCategories,
              setSettings,
              change,
              saveError,
              liveEnabled: live.enabled,
              showLive,
              selectLive: () => {
                setOfflinePreview(false);
                setPage("status");
              },
            }}
          />
        ) : (
          <>
            <StatusDisplay
              decks={shownDecks}
              settings={settings}
              onToggleTime={() =>
                change(
                  "time",
                  settings.time === "remaining" ? "elapsed" : "remaining",
                )
              }
              onZoom={(window) => change("window", window)}
            />
            <footer>
              {showLive ? (
                <>
                  <span className="preview-badge">LIVE CDJ STATUS</span>
                  <span>
                    {live.error ??
                      "Select tracks in BROWSE; playback controls remain on the CDJs"}
                  </span>
                  <small>Time estimate from the beat grid</small>
                </>
              ) : decks.some(Boolean) ? (
                <>
                  <span className="preview-badge">OFFLINE PREVIEW</span>
                  <select
                    aria-label="Scrub preview deck"
                    value={previewDeck}
                    onChange={(e) => setPreviewDeck(+e.target.value)}
                  >
                    <option value={0}>Deck 1</option>
                    <option value={1}>Deck 2</option>
                  </select>
                  <input
                    aria-label="Preview position"
                    type="range"
                    min={0}
                    max={previewDuration}
                    step={0.01}
                    disabled={!preview}
                    value={preview?.position ?? 0}
                    onChange={(e) => scrub(+e.target.value)}
                  />
                  <span>
                    {timeLabel(preview ? preview.position : null)} /{" "}
                    {timeLabel(durationOf(preview))}
                  </span>
                  <small>Local inspection only</small>
                </>
              ) : (
                <>
                  <span className="status-dot" />
                  <span>
                    CDJ status will appear when a live source is connected.
                  </span>
                  <button
                    onClick={() =>
                      setPage(__OLC_EXPERIMENTS__ ? "test" : "config")
                    }
                  >
                    Open offline preview
                  </button>
                </>
              )}
            </footer>
          </>
        )}
      </div>
    </main>
  );
}
createRoot(document.getElementById("root")!).render(<App />);
