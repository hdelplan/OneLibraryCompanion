import { LocalUsb } from "./LocalUsb";
import {
  type SetHistoryState,
  setDate,
  currentSetTrackKeys,
  playedTrackKey,
} from "./setHistoryModel";
import { memo, useEffect, useRef, useState, type CSSProperties } from "react";
import { createPortal } from "react-dom";
import {
  readLibraryDensity,
  densityRows,
  type LibraryDensity,
} from "./libraryDensity";
import {
  activeFilterParams,
  selectedLibrarySource,
  newlyAvailableLocalSource,
  colorPalette,
  duration,
  emptyFilters,
  filterCount,
  folderPath,
  type Filters,
  type LibraryInfo,
  type LibrarySource,
  type LibraryTrack,
  type TrackPage,
} from "./libraryModel";
import "./library.css";
import { LibraryFilterBar } from "./LibraryFilterBar";
import { nextColumnSort } from "./libraryPreferences";
import type { Analysis, LivePlayer } from "./model";
import { LibraryArtwork } from "./LibraryArtwork";
import { LoadControls, useTrackLoader } from "./LoadControls";
import { incompatiblePlayers } from "./trackCompatibility";
import { sameLoadPlayers } from "./libraryLoading";
import { keyCompatibility } from "./keyCompatibility";
import { useTrackWindow } from "./useTrackWindow";
async function request<T>(
  path: string,
  signal?: AbortSignal,
  method = "GET",
): Promise<T> {
  const response = await fetch(path, { signal, method });
  if (!response.ok) throw new Error(await response.text());
  return response.json() as Promise<T>;
}
function filterLabel(
  key: string,
  value: string,
  info: LibraryInfo | null,
  playlistName?: string,
) {
  if (key === "rules") return "Metadata rules";
  if (key === "playlist") return `Playlist: ${playlistName ?? value}`;
  if (key === "tags" || key.startsWith("tagCategory:")) {
    let ids: string[] = [];
    try {
      ids = key === "tags" ? value.split(",") : (JSON.parse(value) as string[]);
    } catch {
      /* Invalid selections are reported by the API. */
    }
    const category = key.startsWith("tagCategory:")
      ? info?.categories.find((c) => String(c.id) === key.split(":")[1])?.name
      : "My Tags";
    return `${category ?? "My Tags"}: ${ids.map((id) => info?.tags.find((t) => String(t.id) === id)?.name ?? id).join(", ")}`;
  }
  if (value.startsWith("[")) {
    try {
      return `${key}: ${(JSON.parse(value) as string[]).map((v) => v || "Not set").join(", ")}`;
    } catch {
      /* Keep original value. */
    }
  }
  return `${key}: ${value}`;
}
type Preset = {
  name: string;
  source: string;
  fingerprint: string;
  filters: Filters;
};
function readPresets(): Preset[] {
  try {
    const data: unknown = JSON.parse(
      localStorage.getItem("pc.library.presets.v1") ?? "[]",
    );
    if (!Array.isArray(data)) return [];
    return data.filter(
      (p): p is Preset =>
        typeof p?.name === "string" &&
        typeof p?.source === "string" &&
        typeof p?.fingerprint === "string" &&
        p.filters &&
        typeof p.filters === "object" &&
        Object.values(p.filters).every((v) => typeof v === "string"),
    );
  } catch {
    return [];
  }
}
function LibraryView({
  toolbarTarget,
  activeKey,
  active,
  filterOrder,
  bpmRange,
  onCategories,
  players,
  directAllowed,
  sourceRequest,
  onPreview,
  onLoaded,
}: {
  toolbarTarget: HTMLDivElement | null;
  activeKey: string | null;
  active: boolean;
  filterOrder: string[];
  bpmRange: { min: number; max: number };
  onCategories: (names: string[]) => void;
  players: LivePlayer[];
  directAllowed: boolean;
  sourceRequest: { id: string } | null;
  onPreview: (analysis: Analysis) => void;
  onLoaded: () => void;
}) {
  const [density, setDensity] = useState<LibraryDensity>(readLibraryDensity);
  const [rowHeight, setRowHeight] = useState(48.5);
  useEffect(() => {
    try {
      localStorage.setItem("pc.library.density.v1", density);
    } catch {
      /* Keep the choice for this session. */
    }
  }, [density]);
  const [showSetHistory, setShowSetHistory] = useState(false);
  const [history, setHistory] = useState<SetHistoryState | null>(null);
  const [selectedSet, setSelectedSet] = useState("");
  const savedSets = [...(history?.sets ?? [])]
    .filter((set) => set.id !== history?.activeId)
    .sort((a, b) => b.startedAt - a.startedAt);
  const currentSet =
    savedSets.find((set) => set.id === selectedSet) ?? savedSets[0];
  useEffect(() => {
    if (!active) return;
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try {
        const next = await request<SetHistoryState>(
          "/api/sets",
          controller.signal,
        );
        if (!controller.signal.aborted)
          setHistory((previous) =>
            previous?.revision === next.revision ? previous : next,
          );
      } catch (e) {
        if (!controller.signal.aborted) setError(String(e));
      }
      if (!controller.signal.aborted)
        timer = setTimeout(() => void poll(), 1500);
    }
    void poll();
    return () => {
      controller.abort();
      clearTimeout(timer);
    };
  }, [active]);
  const playedTracks = currentSetTrackKeys(history);
  const [previewBusy, setPreviewBusy] = useState(false);
  const previewRequest = useRef<AbortController | null>(null);
  const [sources, setSources] = useState<LibrarySource[]>([]);
  const previousSources = useRef<LibrarySource[]>([]);
  const [sourceId, setSourceId] = useState("");
  const [info, setInfo] = useState<LibraryInfo | null>(null);
  const [filters, setFilters] = useState<Filters>({ ...emptyFilters });
  const [page, setPage] = useState<TrackPage | null>(null);
  const [phraseMood, setPhraseMood] = useState("Unavailable");
  const [selected, setSelected] = useState<LibraryTrack | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [presets, setPresets] = useState(readPresets);
  const [presetName, setPresetName] = useState("");
  const [revision, setRevision] = useState(0);
  const attempted = useRef("");
  const source = sources.find((s) => s.id === sourceId);
  const generation = source?.generation;
  const state = source?.state;
  const base = `/api/library/${encodeURIComponent(sourceId)}`;
  useEffect(() => {
    previewRequest.current?.abort();
    setPreviewBusy(false);
    return () => {
      previewRequest.current?.abort();
    };
  }, [sourceId, generation, active]);
  useEffect(() => {
    const controller = new AbortController();
    setPhraseMood("Unavailable");
    if (
      active &&
      selected &&
      selected.available !== false &&
      generation != null
    ) {
      setPhraseMood("Loading…");
      void request<{ mood: string | null }>(
        `${base}/mood/${selected.id}?generation=${generation}`,
        controller.signal,
      )
        .then((result) => {
          if (!controller.signal.aborted)
            setPhraseMood(result.mood ?? "Unavailable");
        })
        .catch(() => {
          if (!controller.signal.aborted) setPhraseMood("Unavailable");
        });
    }
    return () => controller.abort();
  }, [active, base, generation, selected]);

  async function previewTrack(track: LibraryTrack) {
    previewRequest.current?.abort();
    const controller = new AbortController();
    previewRequest.current = controller;
    setPreviewBusy(true);
    setError("");
    try {
      const analysis = await request<Analysis>(
        `${base}/preview/${track.id}?generation=${generation}`,
        controller.signal,
      );
      if (!controller.signal.aborted) onPreview(analysis);
    } catch (e) {
      if (!controller.signal.aborted) setError(String(e));
    } finally {
      if (previewRequest.current === controller) setPreviewBusy(false);
    }
  }
  useEffect(() => {
    if (info?.tagsAvailable) onCategories(info.categories.map((c) => c.name));
  }, [info, onCategories]);
  useEffect(() => {
    if (!active) return;
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try {
        const result = await request<{ sources: LibrarySource[] }>(
          "/api/library/sources",
          controller.signal,
        );
        if (controller.signal.aborted) return;
        const added = newlyAvailableLocalSource(
          previousSources.current,
          result.sources,
        );
        previousSources.current = result.sources;
        setSources(result.sources);
        setSourceId((id) => added ?? selectedLibrarySource(id, result.sources));
      } catch (e) {
        if (!controller.signal.aborted) setError(String(e));
      }
      if (!controller.signal.aborted)
        timer = setTimeout(() => void poll(), 1500);
    }
    void poll();
    return () => {
      controller.abort();
      clearTimeout(timer);
    };
  }, [active, revision]);
  useEffect(() => {
    setFilters((f) => ({
      ...Object.fromEntries(
        Object.entries(f).filter(([key]) => !key.startsWith("tagCategory:")),
      ),
      tags: "",
      playlist: "",
    }));
    setInfo(null);
    setPage(null);
    setSelected(null);
    if (!sourceId) return;
    const controller = new AbortController();
    if (state === "idle") {
      const key = `${sourceId}:${generation}`;
      if (attempted.current !== key) {
        attempted.current = key;
        void request(base + "/refresh", controller.signal, "POST")
          .then(() => setRevision((r) => r + 1))
          .catch((e: unknown) => {
            if (!controller.signal.aborted) {
              attempted.current = "";
              setError(String(e));
            }
          });
      }
    } else if (state === "ready") {
      void request<LibraryInfo>(
        `${base}?generation=${generation}`,
        controller.signal,
      )
        .then(setInfo)
        .catch((e: unknown) => {
          if (!controller.signal.aborted) setError(String(e));
        });
    }
    return () => controller.abort();
  }, [sourceId, generation, state, base]);
  useEffect(() => {
    if (!showSetHistory && (!info || info.generation !== generation)) return;
    if (showSetHistory && !currentSet) {
      setPage(null);
      setSelected(null);
      setLoading(false);
      return;
    }
    const controller = new AbortController();
    setLoading(true);
    setPage(null);
    setSelected(null);
    const timer = setTimeout(() => {
      const params = new URLSearchParams({
        ...activeFilterParams(
          showSetHistory ? { ...filters, playlist: "" } : filters,
        ),
        ...(showSetHistory && currentSet ? { set: currentSet.id } : {}),
        generation: String(generation),
        all: "true",
      });
      const offlineSet =
        showSetHistory &&
        currentSet &&
        (!source?.available ||
          state !== "ready" ||
          !info ||
          info.generation !== generation);
      const path = offlineSet
        ? `/api/sets/${encodeURIComponent(currentSet.id)}/tracks?${params}`
        : `${base}/tracks?${params}`;
      void request<TrackPage>(path, controller.signal)
        .then((result) => {
          if (controller.signal.aborted) return;
          setPage(result);
          setError("");
        })
        .catch((e: unknown) => {
          if (!controller.signal.aborted) setError(String(e));
        })
        .finally(() => {
          if (!controller.signal.aborted) setLoading(false);
        });
    }, 180);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, [
    info,
    filters,
    base,
    generation,
    showSetHistory,
    currentSet,
    source?.available,
    state,
  ]);
  useEffect(() => {
    setFilters((current) => {
      const next = { ...current };
      for (const key of ["bpmMin", "bpmMax"]) {
        if (
          next[key] &&
          (Number(next[key]) < bpmRange.min || Number(next[key]) > bpmRange.max)
        )
          next[key] = "";
      }
      return next.bpmMin === current.bpmMin && next.bpmMax === current.bpmMax
        ? current
        : next;
    });
  }, [bpmRange.min, bpmRange.max]);
  function change(field: string, value: string) {
    setFilters((f) => activeFilterParams({ ...f, [field]: value }));
  }
  function switchSource(id: string) {
    setSourceId(id);
    setFilters({ ...emptyFilters });
    setError("");
  }
  useEffect(() => {
    if (!sourceRequest) return;
    switchSource(sourceRequest.id);
    setRevision((r) => r + 1);
  }, [sourceRequest]);
  async function refresh() {
    setError("");
    setPage(null);
    setSelected(null);
    setInfo(null);
    try {
      await request(base + "/refresh", undefined, "POST");
      setRevision((r) => r + 1);
    } catch (e) {
      setError(String(e));
    }
  }
  function savePreset() {
    if (!presetName.trim()) return;
    if (!info) return;
    const next = [
      ...presets.filter(
        (p) => p.name !== presetName.trim() || p.source !== sourceId,
      ),
      {
        name: presetName.trim(),
        source: sourceId,
        filters: { ...filters },
        fingerprint: info.fingerprint,
      },
    ];
    try {
      localStorage.setItem("pc.library.presets.v1", JSON.stringify(next));
      setPresets(next);
      setPresetName("");
    } catch {
      setError("Could not save browser preset in this browser.");
    }
  }
  const nodes = info?.playlists ?? [];
  const currentNode = showSetHistory
    ? undefined
    : nodes.find((p) => String(p.id) === filters.playlist);
  const folderId = currentNode?.folder ? currentNode.id : currentNode?.parentId;
  const children = nodes
    .filter((p) =>
      folderId
        ? p.parentId === folderId && p.id !== folderId
        : !nodes.some((parent) => parent.id === p.parentId),
    )
    .sort((a, b) => a.order - b.order || a.name.localeCompare(b.name));
  const breadcrumb = currentNode ? folderPath(nodes, currentNode.id) : [];
  const displayFilters = showSetHistory
    ? { ...filters, playlist: "" }
    : filters;
  const count = filterCount(displayFilters);
  const loader = useTrackLoader(source, active, onLoaded);
  const window = useTrackWindow(
    page?.tracks.length ?? 0,
    page,
    active,
    rowHeight,
  );
  useEffect(() => {
    const element = window.ref.current;
    if (!active || !element) return;
    const resize = () => {
      const header = element.querySelector("thead") as HTMLElement | null;
      if (!header) return;
      // Layout pixels are independent of the overall iPad/browser screen scale.
      const available = element.clientHeight - header.offsetHeight - 0.5;
      setRowHeight(
        Math.max(
          36.1,
          Math.floor((available / densityRows[density]) * 64) / 64,
        ),
      );
    };
    const observer = new ResizeObserver(resize);
    observer.observe(element);
    resize();
    return () => observer.disconnect();
  }, [active, density, page, window.ref]);
  const visibleTracks = page?.tracks.slice(window.start, window.end) ?? [];
  return (
    <section
      className={`library-screen density-${density}`}
      style={{ "--library-row-height": `${rowHeight}px` } as CSSProperties}
      aria-label="Rekordbox library"
      hidden={!active}
    >
      {active &&
        toolbarTarget &&
        createPortal(
          <div className="library-top-controls">
            <LocalUsb />
            <select
              aria-label="Library source"
              value={sourceId}
              onChange={(e) => switchSource(e.target.value)}
            >
              <option value="" disabled>
                Select USB source
              </option>
              {sources
                .filter((s) => s.available)
                .map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.label}
                    {!s.available ? " · unavailable" : ""}
                  </option>
                ))}
            </select>
            <span
              className={`library-state ${state === "ready" ? "ready" : ""}`}
            >
              {state === "ready"
                ? `${info?.count ?? source?.count ?? 0} TRACKS`
                : (state?.toUpperCase() ?? "NO USB")}
            </span>
            <button
              disabled={!source?.available || state === "loading"}
              onClick={() => void refresh()}
            >
              Refresh USB
            </button>
          </div>,
          toolbarTarget,
        )}
      {loader.message && (
        <p className="library-load-message" role="status">
          {loader.message}
        </p>
      )}
      <div className="library-body">
        <aside className="library-sidebar">
          <div className="library-section-title">COLLECTION</div>
          <button
            className={!showSetHistory && !filters.playlist ? "chosen" : ""}
            onClick={() => {
              setShowSetHistory(false);
              change("playlist", "");
            }}
          >
            All tracks <span>{info?.count ?? "—"}</span>
          </button>
          {currentNode?.folder && (
            <button
              onClick={() =>
                change(
                  "playlist",
                  nodes.some((n) => n.id === currentNode.parentId)
                    ? String(currentNode.parentId)
                    : "",
                )
              }
            >
              ← Parent folder
            </button>
          )}
          <button
            className="library-history-toggle"
            aria-pressed={showSetHistory}
            onClick={() => {
              setShowSetHistory((value) => !value);
              setFilters((value) => ({
                ...value,
                sort: "playlist",
                direction: "",
              }));
              setPage(null);
              setSelected(null);
            }}
          >
            PLAYLISTS / SET HISTORY
          </button>

          <div className="library-playlists">
            {showSetHistory
              ? savedSets.map((set) => (
                  <button
                    key={set.id}
                    className={`library-set-entry ${currentSet?.id === set.id ? "chosen" : ""}`}
                    title={`${set.title} · ${setDate(set)}`}
                    onClick={() => {
                      setSelectedSet(set.id);
                      setFilters((value) => ({
                        ...value,
                        sort: "playlist",
                        direction: "",
                      }));
                    }}
                  >
                    <strong>{set.title}</strong>
                    <span>{setDate(set, true)}</span>
                    <small>{set.order.length} tracks</small>
                  </button>
                ))
              : children.map((p) => (
                  <button
                    key={p.id}
                    className={currentNode?.id === p.id ? "chosen" : ""}
                    onClick={() => change("playlist", String(p.id))}
                  >
                    <span>
                      {p.folder ? "▸ " : "♫ "}
                      {p.name}
                    </span>
                    <small>{p.folder ? "FOLDER" : p.count}</small>
                  </button>
                ))}
            {showSetHistory && !savedSets.length && (
              <p className="library-muted">
                {history ? "No saved sets yet" : "Reading set history…"}
              </p>
            )}
            {!showSetHistory && info && children.length === 0 && (
              <p className="library-muted">No playlists here</p>
            )}
          </div>
          <div className="library-section-title">SAVED FILTERS</div>
          {presets
            .filter((p) => p.source === sourceId)
            .map((p) => (
              <button
                key={p.name}
                onClick={() => {
                  if (p.fingerprint !== info?.fingerprint) {
                    setError(
                      "This preset belongs to a different export. Refresh or recreate it for this USB.",
                    );
                    return;
                  }
                  setFilters({ ...p.filters });
                }}
              >
                {p.name}
              </button>
            ))}
          <div className="library-save">
            <input
              aria-label="Preset name"
              placeholder="Name these filters"
              value={presetName}
              onChange={(e) => setPresetName(e.target.value)}
            />
            <button disabled={!presetName.trim() || !info} onClick={savePreset}>
              Save
            </button>
          </div>
          <small>
            Saved locally for this export, including playlists and My Tags.
          </small>
        </aside>
        <div className="library-content">
          <div className="library-search">
            <input
              type="search"
              aria-label="Search library"
              placeholder="Search title, artist, album, comments…"
              value={filters.q ?? ""}
              onChange={(e) => change("q", e.target.value)}
            />
            <button
              onClick={() => {
                setFilters({ ...emptyFilters });
              }}
            >
              Clear filters{count ? ` (${count})` : ""}
            </button>
          </div>
          <LibraryFilterBar
            density={density}
            onDensityChange={setDensity}
            order={filterOrder}
            bpmRange={bpmRange}
            info={info}
            filters={filters}
            change={change}
          />
          <div className="library-results-bar">
            <span>
              {showSetHistory
                ? (currentSet?.title ?? "Set history")
                : breadcrumb.length
                  ? breadcrumb.map((p) => p.name).join(" / ")
                  : "All tracks"}{" "}
              <small>· {page?.total ?? "—"} matches</small>
            </span>
            {count > 0 && (
              <div className="library-chips">
                {Object.entries(displayFilters)
                  .filter(
                    ([key, value]) =>
                      value &&
                      value !== "[]" &&
                      !["sort", "direction", "tagMode"].includes(key) &&
                      !key.endsWith(":mode"),
                  )
                  .map(([key, value]) => (
                    <button
                      key={key}
                      onClick={() => change(key, "")}
                      aria-label={`Remove ${key} filter`}
                    >
                      {filterLabel(key, value, info, currentNode?.name)} ×
                    </button>
                  ))}
              </div>
            )}
          </div>
          {(error || source?.error) && (
            <p role="alert" className="library-error">
              {error || source?.error}
            </p>
          )}
          <div
            className="library-table-wrap"
            role="region"
            aria-label="Browse tracks"
            tabIndex={0}
            ref={window.ref}
            onScroll={window.onScroll}
          >
            {!active ? null : !showSetHistory && !sourceId ? (
              <div className="library-empty">
                <h2>Connect a Rekordbox USB</h2>
                <p>
                  Insert an exported USB into a linked CDJ. Its library will
                  appear here.
                </p>
                <p>A configured local export is also supported.</p>
              </div>
            ) : !showSetHistory && !source?.available ? (
              <div className="library-empty">
                <h2>USB unavailable</h2>
                <p>Reconnect the source player or insert the USB again.</p>
              </div>
            ) : !showSetHistory && (state === "loading" || state === "idle") ? (
              <div className="library-empty">
                <h2>Reading USB library…</h2>
                <p>
                  Loading track metadata and My Tags. CDJ monitoring continues.
                </p>
              </div>
            ) : showSetHistory && !currentSet ? (
              <div className="library-empty">
                Choose a saved set on the left.
              </div>
            ) : currentNode?.folder ? (
              <div className="library-empty">
                <h2>{currentNode.name}</h2>
                <p>Choose a child playlist or folder on the left.</p>
              </div>
            ) : loading ? (
              <div className="library-empty">Finding tracks…</div>
            ) : page?.tracks.length ? (
              <table
                className="library-table"
                aria-rowcount={(page?.total ?? 0) + 1}
              >
                <thead>
                  <tr>
                    {[
                      ["color", "COLOR"],
                      ["artwork", "ARTWORK"],
                      ["title", "TITLE / ARTIST"],
                      ["genre", "GENRE"],
                      ["bpm", "BPM"],
                      ["key", "KEY"],
                      ["rating", "RATING"],
                      ["duration", "TIME"],
                    ].map(([field, label]) =>
                      field === "artwork" ? (
                        <th key={field} aria-label="Artwork">
                          ARTWORK
                        </th>
                      ) : (
                        <th
                          key={field}
                          aria-sort={
                            filters.sort === field
                              ? filters.direction === "desc"
                                ? "descending"
                                : "ascending"
                              : "none"
                          }
                        >
                          <button
                            onClick={() => {
                              setFilters((current) =>
                                nextColumnSort(current, field),
                              );
                            }}
                            aria-label={`Sort by ${field}`}
                          >
                            {label}
                            {filters.sort === field
                              ? filters.direction === "desc"
                                ? " ↓"
                                : " ↑"
                              : ""}
                          </button>
                        </th>
                      ),
                    )}
                    <th className="library-load-heading">
                      {source?.direct && directAllowed
                        ? "PREVIEW / LOAD"
                        : "LOAD"}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {window.start > 0 && (
                    <tr aria-hidden="true" className="library-spacer">
                      <td
                        colSpan={9}
                        style={{ height: window.start * rowHeight }}
                      />
                    </tr>
                  )}
                  {visibleTracks.map((track, index) => (
                    <tr
                      key={`${track.id}:${track.entry}`}
                      aria-rowindex={window.start + index + 2}
                      className={`${selected?.entry === track.entry ? "selected" : ""} ${playedTracks.has(playedTrackKey(track) ?? "") ? "played-in-current-set" : ""}`}
                    >
                      <td>
                        <span
                          className="library-color"
                          style={{
                            background:
                              colorPalette[track.colorId] ?? colorPalette[0],
                          }}
                          title={track.color || "No color"}
                          aria-label={track.color || "No color"}
                        />
                      </td>
                      <td>
                        <LibraryArtwork
                          key={`${base}:${generation}:${track.id}`}
                          track={track}
                          base={base}
                          generation={generation}
                        />
                      </td>
                      <td>
                        <button
                          className="library-track-title"
                          onClick={() => setSelected(track)}
                        >
                          <span className="library-title-line">
                            <span className="library-title-text">
                              {track.title || "Untitled"}
                            </span>
                            {incompatiblePlayers(track, players).length > 0 && (
                              <span
                                className="library-unsupported"
                                title={`Unsupported on ${incompatiblePlayers(
                                  track,
                                  players,
                                )
                                  .map((p) => `CDJ${p.number} (${p.name})`)
                                  .join(", ")}`}
                              >
                                UNSUPPORTED FORMAT
                              </span>
                            )}
                          </span>
                          <small>{track.artist || "Unknown artist"}</small>
                          {track.available === false && (
                            <small className="library-unavailable">
                              {source?.available && state === "ready"
                                ? "Unavailable on selected USB"
                                : "Connect a matching USB to load"}
                            </small>
                          )}
                        </button>
                      </td>
                      <td>{track.genre || "—"}</td>
                      <td>{track.bpm ? track.bpm.toFixed(2) : "—"}</td>
                      <td
                        className={`library-key-${keyCompatibility(activeKey, track.key) ?? "neutral"}`}
                        title={
                          keyCompatibility(activeKey, track.key) ===
                          "compatible"
                            ? `Compatible with active key ${activeKey}`
                            : keyCompatibility(activeKey, track.key) === "semi"
                              ? `Semi-compatible with active key ${activeKey}`
                              : undefined
                        }
                      >
                        {track.key || "—"}
                      </td>
                      <td
                        className="library-stars"
                        aria-label={`${track.rating} stars`}
                      >
                        {"★".repeat(Math.min(5, track.rating)) || "—"}
                      </td>
                      <td>{duration(track.duration)}</td>
                      <td>
                        {source?.direct ? (
                          <>
                            {directAllowed && (
                              <button
                                disabled={
                                  previewBusy || track.available === false
                                }
                                onClick={() => void previewTrack(track)}
                              >
                                {previewBusy ? "Loading…" : "Waveform"}
                              </button>
                            )}
                            <LoadControls
                              track={track}
                              source={{ ...source, loadable: true }}
                              players={players.filter(
                                (p) =>
                                  p.sourceLabel === "Direct IP · USB" ||
                                  p.sourceLabel === "Local USB",
                              )}
                              loader={loader}
                            />
                          </>
                        ) : (
                          <LoadControls
                            track={track}
                            source={source}
                            players={players}
                            loader={loader}
                          />
                        )}
                      </td>
                    </tr>
                  ))}
                  {window.end < page.tracks.length && (
                    <tr aria-hidden="true" className="library-spacer">
                      <td
                        colSpan={9}
                        style={{
                          height: (page.tracks.length - window.end) * rowHeight,
                        }}
                      />
                    </tr>
                  )}
                </tbody>
              </table>
            ) : (showSetHistory || state === "ready") && !error ? (
              <div className="library-empty">
                <h2>No matching tracks</h2>
                <p>Try fewer filters or choose another playlist.</p>
              </div>
            ) : null}
          </div>
        </div>
      </div>
      {source?.id.startsWith("local-usb:") && !source.loadable && (
        <p className="library-load-message" role="status">
          Local USB loading requires Manual IP connections in MENU.
        </p>
      )}
      <div className="library-details">
        {active && selected ? (
          <div className="library-detail-layout">
            <LibraryArtwork
              key={`${base}:${generation}:${selected.id}:large`}
              track={selected}
              base={base}
              generation={generation}
              large
            />
            <div className="library-detail-content">
              <div>
                <strong>{selected.title}</strong>
                <span>
                  {[selected.artist, selected.album]
                    .filter(Boolean)
                    .join(" · ")}
                </span>
                <button onClick={() => setSelected(null)}>Close details</button>
              </div>
              <dl>
                <div>
                  <dt>Mood</dt>
                  <dd title="Rekordbox phrase-analysis mood; determines the phrase color palette.">
                    {phraseMood}
                  </dd>
                </div>
                {Object.entries(selected)
                  .filter(
                    ([key]) =>
                      ![
                        "entry",
                        "myTags",
                        "title",
                        "artist",
                        "album",
                        "colorId",
                      ].includes(key),
                  )
                  .map(([key, value]) => (
                    <div key={key}>
                      <dt>{key.replace(/([A-Z])/g, " $1")}</dt>
                      <dd>
                        {String(value === "" || value == null ? "—" : value)}
                      </dd>
                    </div>
                  ))}
                <div>
                  <dt>My Tags</dt>
                  <dd>
                    {selected.myTags
                      ?.map((t) => `${t.category}: ${t.name}`)
                      .join(" · ") || "—"}
                  </dd>
                </div>
              </dl>
            </div>
          </div>
        ) : (
          <p>
            Select a track to inspect all exported metadata, color, rating, and
            My Tags.<span>BROWSE</span>
          </p>
        )}
      </div>
    </section>
  );
}

// Position packets must not rebuild a hidden browser or hundreds of track rows.
export const Library = memo(
  LibraryView,
  (a, b) =>
    a.toolbarTarget === b.toolbarTarget &&
    a.active === b.active &&
    a.activeKey === b.activeKey &&
    a.directAllowed === b.directAllowed &&
    a.sourceRequest === b.sourceRequest &&
    a.onPreview === b.onPreview &&
    a.onLoaded === b.onLoaded &&
    a.filterOrder === b.filterOrder &&
    a.bpmRange.min === b.bpmRange.min &&
    a.bpmRange.max === b.bpmRange.max &&
    a.onCategories === b.onCategories &&
    sameLoadPlayers(a.players, b.players),
);
