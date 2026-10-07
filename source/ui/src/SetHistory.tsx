import { SwipeSet } from "./SwipeSet";
import { useEffect, useRef, useState } from "react";
import {
  type DJSet,
  type ExportFormat,
  type SetHistoryState,
  orderedTracks,
  setDate,
} from "./setHistoryModel";
import { download, emailDraft, exportFile } from "./setHistoryExport";
import type { LibrarySource } from "./libraryModel";
import "./setHistory.css";
async function request(url: string, body?: unknown) {
  const response = await fetch(
    url,
    body
      ? {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        }
      : { cache: "no-store" },
  );
  if (!response.ok) throw new Error(await response.text());
  return response.json();
}
function Artwork({ name }: { name: string | null }) {
  const [failed, setFailed] = useState(false);
  return (
    <div className="library-artwork">
      {name && !failed ? (
        <img
          src={`/api/sets/artwork/${encodeURIComponent(name)}`}
          alt=""
          loading="lazy"
          onError={() => setFailed(true)}
        />
      ) : (
        <span role="img" aria-label="Artwork unavailable">
          ♪
        </span>
      )}
    </div>
  );
}
function Details({
  set,
  save,
  onDirty,
  sessionLabel,
}: {
  set: DJSet;
  save: (body: unknown) => Promise<void>;
  onDirty: (dirty: boolean) => void;
  sessionLabel: string;
}) {
  const [title, setTitle] = useState(set.title);
  const [location, setLocation] = useState(set.location),
    [comment, setComment] = useState(set.comment);
  useEffect(
    () =>
      onDirty(
        title !== set.title ||
          location !== set.location ||
          comment !== set.comment,
      ),
    [title, location, comment, set.title, set.location, set.comment, onDirty],
  );
  return (
    <div className="set-overview">
      <div className="set-heading">
        <span className="library-eyebrow">{sessionLabel}</span>
        <input
          className="set-name"
          aria-label="Set name"
          title="Edit set name"
          value={title}
          maxLength={200}
          onChange={(e) => setTitle(e.target.value)}
          onBlur={() => {
            const name = title.trim() || set.title;
            setTitle(name);
            if (name !== set.title)
              void save({ action: "metadata", id: set.id, title: name });
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") e.currentTarget.blur();
          }}
        />
        <p>
          {setDate(set)}
          {set.dateOnly
            ? " · Time unavailable"
            : set.endedAt
              ? ` – ${new Date(set.endedAt).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" })}`
              : " · In progress"}
        </p>
        <div className="set-count">
          <strong>{set.order.length}</strong>
          <span>TRACKS</span>
        </div>
      </div>
      <div className="set-details">
        <label>
          <span>LOCATION</span>
          <input
            placeholder="Add a venue or location"
            value={location}
            maxLength={200}
            onChange={(e) => setLocation(e.target.value)}
            onBlur={() => {
              if (location !== set.location)
                void save({ action: "metadata", id: set.id, location });
            }}
          />
        </label>
        <label>
          <span>COMMENT</span>
          <textarea
            rows={3}
            aria-label="COMMENT"
            placeholder="Notes about this set"
            value={comment}
            maxLength={4000}
            onChange={(e) => setComment(e.target.value)}
            onBlur={() => {
              if (comment !== set.comment)
                void save({ action: "metadata", id: set.id, comment });
            }}
          />
        </label>
      </div>
    </div>
  );
}
export function SetHistory({ active }: { active: boolean }) {
  const [state, setState] = useState<SetHistoryState | null>(null),
    [selected, setSelected] = useState<string | null>(null);
  const [connectionError, setConnectionError] = useState("");
  const [error, setError] = useState(""),
    [notice, setNotice] = useState(""),
    [busy, setBusy] = useState(false);
  const [sources, setSources] = useState<LibrarySource[]>([]),
    [source, setSource] = useState("");
  const [importOpen, setImportOpen] = useState(false),
    [format, setFormat] = useState<ExportFormat>("txt"),
    [scope, setScope] = useState("selected");
  const [prepared, setPrepared] = useState<File | null>(null),
    [exportBusy, setExportBusy] = useState(false);
  const [edit, setEdit] = useState(false),
    [detailsDirty, setDetailsDirty] = useState(false);
  const latest = useRef(-1);
  const accept = (next: SetHistoryState) => {
    if (next.revision >= latest.current) {
      latest.current = next.revision;
      setState(next);
    }
  };
  useEffect(() => {
    if (!active) return;
    let closed = false;
    const poll = async () => {
      try {
        const next = await request("/api/sets");
        if (!closed) {
          accept(next);
          setConnectionError("");
        }
      } catch (e) {
        if (!closed) setConnectionError(String(e));
      }
    };
    void poll();
    const timer = setInterval(() => void poll(), 1000);
    return () => {
      closed = true;
      clearInterval(timer);
    };
  }, [active]);
  const sorted = [...(state?.sets ?? [])].sort(
    (a, b) => b.startedAt - a.startedAt,
  );
  const current = sorted.find((s) => s.id === state?.activeId);
  const past = sorted.filter((s) => s.id !== state?.activeId);
  const set = sorted.find((s) => s.id === selected) ?? current ?? past[0];
  const exportSets =
    scope === "all"
      ? past
      : scope === "last"
        ? past.slice(0, 1)
        : set
          ? [set]
          : [];
  // Prepare files ahead of the click so Web Share keeps the user's activation.
  const signature = JSON.stringify(exportSets);
  useEffect(() => {
    if (!active || !exportSets.length) {
      setPrepared(null);
      return;
    }
    let cancelled = false;
    setExportBusy(true);
    setPrepared(null);
    const timer = setTimeout(() => {
      void exportFile(JSON.parse(signature), format)
        .then((file) => {
          if (!cancelled) setPrepared(file);
        })
        .catch((e) => {
          if (!cancelled) setError(String(e));
        })
        .finally(() => {
          if (!cancelled) setExportBusy(false);
        });
    }, 250);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [signature, format, active]);
  async function command(body: unknown) {
    setBusy(true);
    setError("");
    try {
      accept(await request("/api/sets", body));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  async function cancelSet() {
    if (!current) return;
    const confirm = (set: DJSet) =>
      window.confirm(
        `Cancel “${set.title}”?\n\nThis will discard the current set and its ${set.events.length} recorded track${set.events.length === 1 ? "" : "s"}. It will not be saved in history.`,
      );
    let confirmed = false;
    if (current.events.length) {
      if (!confirm(current)) return;
      confirmed = true;
    }
    setBusy(true);
    setError("");
    try {
      try {
        accept(
          await request("/api/sets", {
            action: "cancel",
            id: current.id,
            confirmed,
          }),
        );
      } catch (e) {
        if (!String(e).includes("CANCEL_CONFIRMATION_REQUIRED")) throw e;
        const fresh: SetHistoryState = await request("/api/sets");
        accept(fresh);
        const latest = fresh.sets.find(
          (set) => set.id === current.id && set.id === fresh.activeId,
        );
        if (!latest || !confirm(latest)) return;
        accept(
          await request("/api/sets", {
            action: "cancel",
            id: current.id,
            confirmed: true,
          }),
        );
      }
      setSelected(null);
      setEdit(false);
      setNotice("Set canceled. It was not saved in history.");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  async function start() {
    await command({ action: "start" });
    setSelected(null);
    setEdit(false);
  }
  async function loadSources() {
    setImportOpen(true);
    try {
      const result = await request("/api/library/sources");
      const items: LibrarySource[] = result.sources.filter(
        (s: LibrarySource) => s.available,
      );
      setSources(items);
      setSource(items[0]?.id ?? "");
    } catch (e) {
      setError(String(e));
    }
  }
  async function importHistory() {
    setBusy(true);
    setError("");
    try {
      let item = sources.find((s) => s.id === source);
      if (!item) throw new Error("Choose an available source.");
      if (item.state !== "ready") {
        await request(`/api/library/${encodeURIComponent(source)}/refresh`, {});
        const end = Date.now() + 65_000;
        while (Date.now() < end) {
          await new Promise((r) => setTimeout(r, 500));
          const result = await request("/api/library/sources");
          item = result.sources.find((s: LibrarySource) => s.id === source);
          if (!item?.available) throw new Error("Source disconnected.");
          if (item.state === "ready") break;
          if (item.state === "error")
            throw new Error(item.error ?? "Could not read source");
        }
      }
      if (item?.state !== "ready")
        throw new Error("Library read timed out. Retry when ready.");
      accept(
        await request("/api/sets/import", {
          source,
          generation: item.generation,
        }),
      );
      setImportOpen(false);
      setNotice(
        "History import complete. Existing imports are kept without duplicates.",
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  async function share() {
    if (!prepared) return;
    try {
      if (navigator.canShare?.({ files: [prepared] })) {
        await navigator.share({ files: [prepared], title: "Set history" });
      } else {
        download(prepared);
        setNotice(
          "File downloaded. Share it from Files, Finder or your mail app.",
        );
      }
    } catch (e) {
      if (!(e instanceof DOMException && e.name === "AbortError"))
        setError(String(e));
    }
  }
  if (!active) return null;
  return (
    <section className="set-screen" aria-label="Set history">
      <header className="set-toolbar">
        <div>
          <span className="library-eyebrow">YOUR SESSIONS, IN ORDER</span>
          <h1>SET HISTORY</h1>
        </div>
        <span className={`set-recording ${state?.recording ? "on" : ""}`}>
          <i />
          {state?.recording
            ? "RECORDING"
            : current
              ? "RECOVERED SET"
              : "READY FOR YOUR NEXT SET"}
        </span>
        <button disabled={busy} onClick={() => void loadSources()}>
          Import history
        </button>
        {current && (
          <button
            className="set-cancel"
            disabled={busy}
            onClick={() => void cancelSet()}
          >
            Cancel set
          </button>
        )}
        {current ? (
          <button
            className="set-primary"
            disabled={busy}
            onClick={() => {
              setSelected(current.id);
              void command({
                action: state?.recording ? "finish" : "resume",
                id: current.id,
              });
            }}
          >
            {state?.recording ? "Finish & save" : "Resume set"}
          </button>
        ) : (
          <button
            className="set-primary"
            disabled={busy || !state || !!state.error}
            onClick={() => void start()}
          >
            ＋ Start set
          </button>
        )}
      </header>
      {(error || connectionError || state?.error) && (
        <div className="set-alert" role="alert">
          {error || connectionError || state?.error}
        </div>
      )}
      {notice && (
        <div className="set-notice" role="status">
          {notice}
          <button onClick={() => setNotice("")} aria-label="Dismiss message">
            ×
          </button>
        </div>
      )}
      {importOpen && (
        <div className="set-import">
          <div>
            <strong>Import from Rekordbox</strong>
            <p>
              Import complete histories whose names contain a date in YYYY-MM-DD
              format. Undated or incomplete histories are skipped.
            </p>
          </div>
          <select
            aria-label="History source"
            value={source}
            onChange={(e) => setSource(e.target.value)}
          >
            {!sources.length && (
              <option value="">No USB or local export available</option>
            )}
            {sources.map((s) => (
              <option key={s.id} value={s.id}>
                {s.label}
              </option>
            ))}
          </select>
          <button
            disabled={busy || !source}
            onClick={() => void importHistory()}
          >
            {busy ? "Reading…" : "Import history"}
          </button>
          <button disabled={busy} onClick={() => setImportOpen(false)}>
            Cancel
          </button>
        </div>
      )}
      <div className="set-body">
        <aside className="set-sidebar">
          <div className="library-section-title">CURRENT SET</div>
          {current ? (
            <SwipeSet
              title={current.title}
              chosen={set?.id === current.id}
              disabled={busy}
              onDelete={() =>
                void command({
                  action: "delete",
                  id: current.id,
                  confirmed: true,
                })
              }
              onSelect={() => {
                setSelected(current.id);
                setEdit(false);
              }}
            >
              <strong>
                {state?.recording ? "● Recording" : "Interrupted set"}
              </strong>
              <span>{setDate(current, true)}</span>
              <small>{current.order.length} tracks</small>
            </SwipeSet>
          ) : (
            <div className="set-sidebar-empty">
              Start a set to capture your next session.
            </div>
          )}
          <div className="set-past-label">
            <span className="library-section-title">PAST SETS</span>
            <span>{past.length}</span>
          </div>
          {past.map((item) => (
            <SwipeSet
              key={item.id}
              title={item.title}
              chosen={set?.id === item.id}
              disabled={busy}
              onDelete={() =>
                void command({ action: "delete", id: item.id, confirmed: true })
              }
              onSelect={() => {
                setSelected(item.id);
                setEdit(false);
              }}
            >
              <strong>{item.title}</strong>
              <span>{setDate(item, true)}</span>
              <small>
                {item.order.length} tracks
                {item.origin === "sample" && <em>SAMPLE</em>}
                {item.origin === "imported" && <em>IMPORTED</em>}
              </small>
            </SwipeSet>
          ))}
          {!past.length && (
            <div className="set-sidebar-empty">
              Finished sets will appear here.
            </div>
          )}
        </aside>
        <div className="set-main">
          {set ? (
            <>
              <Details
                key={set.id}
                set={set}
                save={command}
                onDirty={setDetailsDirty}
                sessionLabel={
                  set.id === current?.id
                    ? "CURRENT SESSION"
                    : set.origin === "sample"
                      ? "SAMPLE SESSION"
                      : "SAVED SESSION"
                }
              />
              {set.recovered && (
                <div className="set-recovery">
                  This set was interrupted. Saved tracks are intact; resume to
                  start fresh 45-second timers.{" "}
                  <button
                    disabled={busy}
                    onClick={() =>
                      void command({ action: "finish", id: set.id })
                    }
                  >
                    Finish & save
                  </button>
                </div>
              )}
              <div className="set-track-toolbar">
                <span>
                  TRACKLIST{" "}
                  <small>
                    {set.origin === "sample"
                      ? "Sample tracklist"
                      : set.origin === "imported"
                        ? "Original Rekordbox order"
                        : "Qualifies after more than 45 seconds of continuous playback"}
                  </small>
                </span>
                <button onClick={() => setEdit(!edit)}>
                  {edit ? "Done editing" : "Edit tracklist"}
                </button>
                {edit && (
                  <button
                    disabled={busy}
                    onClick={() =>
                      void command({ action: "restore", id: set.id })
                    }
                  >
                    Restore original
                  </button>
                )}
              </div>
              <div className="set-table-wrap">
                <table className="set-table">
                  <thead>
                    <tr>
                      <th>#</th>
                      <th>ART</th>
                      <th>TITLE / ARTIST</th>
                      <th>KEY</th>
                      <th>RATING</th>
                      {edit && <th>EDIT</th>}
                    </tr>
                  </thead>
                  <tbody>
                    {orderedTracks(set).map((event, i) => (
                      <tr key={event.id}>
                        <td className="set-sequence">
                          {String(i + 1).padStart(2, "0")}
                        </td>
                        <td>
                          <Artwork
                            key={event.track.artwork ?? event.id}
                            name={event.track.artwork}
                          />
                        </td>
                        <td>
                          <strong className="set-track-title">
                            {event.track.title || "Untitled"}
                          </strong>
                          <small>
                            {event.track.artist || "Unknown artist"}
                          </small>
                        </td>
                        <td>
                          <span className="set-key">
                            {event.track.key || "—"}
                          </span>
                        </td>
                        <td
                          className="library-stars"
                          aria-label={`${event.track.rating} out of 5 stars`}
                        >
                          {"★".repeat(event.track.rating)}
                          <span className="set-unrated">
                            {"☆".repeat(5 - event.track.rating)}
                          </span>
                        </td>
                        {edit && (
                          <td className="set-row-actions">
                            <button
                              disabled={busy || i === 0}
                              aria-label={`Move ${event.track.title} up`}
                              onClick={() =>
                                void command({
                                  action: "move",
                                  id: set.id,
                                  entry: event.id,
                                  delta: -1,
                                })
                              }
                            >
                              ↑
                            </button>
                            <button
                              disabled={busy || i === set.order.length - 1}
                              aria-label={`Move ${event.track.title} down`}
                              onClick={() =>
                                void command({
                                  action: "move",
                                  id: set.id,
                                  entry: event.id,
                                  delta: 1,
                                })
                              }
                            >
                              ↓
                            </button>
                            <button
                              disabled={busy}
                              aria-label={`Remove ${event.track.title}`}
                              onClick={() =>
                                void command({
                                  action: "remove",
                                  id: set.id,
                                  entry: event.id,
                                })
                              }
                            >
                              ×
                            </button>
                          </td>
                        )}
                      </tr>
                    ))}
                  </tbody>
                </table>
                {!set.order.length && (
                  <div className="set-empty">
                    <h3>
                      {set.id === current?.id
                        ? "Waiting for the first track"
                        : "No tracks in this list"}
                    </h3>
                    <p>
                      {set.id === current?.id
                        ? "Play a track continuously for more than 45 seconds on a connected CDJ."
                        : "Use Restore original while editing to recover removed entries."}
                    </p>
                    {state?.pending.map((p) => (
                      <span className="set-timer" key={p.deck}>
                        CDJ{p.deck} · {p.seconds} / 45 sec
                      </span>
                    ))}
                  </div>
                )}
              </div>
              <div className="set-export">
                <span>EXPORT</span>
                <select
                  aria-label="Export sets"
                  value={scope}
                  onChange={(e) => setScope(e.target.value)}
                >
                  <option value="selected">Selected set</option>
                  <option value="last" disabled={!past.length}>
                    Last finished set
                  </option>
                  <option value="all" disabled={!past.length}>
                    All past sets ({past.length})
                  </option>
                </select>
                <select
                  aria-label="Export format"
                  value={format}
                  onChange={(e) => setFormat(e.target.value as ExportFormat)}
                >
                  <option value="txt">Text (.txt)</option>
                  <option value="csv">CSV (.csv)</option>
                  <option value="pdf">PDF (.pdf)</option>
                </select>
                <button
                  disabled={!prepared || exportBusy || busy || detailsDirty}
                  onClick={() => {
                    if (prepared) download(prepared);
                  }}
                >
                  ↓ Save file
                </button>
                <button
                  disabled={!prepared || exportBusy || busy || detailsDirty}
                  onClick={() => void share()}
                >
                  Share…
                </button>
                <button
                  disabled={!prepared || exportBusy || busy || detailsDirty}
                  onClick={() => {
                    if (prepared) {
                      emailDraft(prepared, exportSets);
                      setNotice(
                        "Export downloaded and email draft requested. For CSV/PDF, attach the downloaded file in your mail app.",
                      );
                    }
                  }}
                >
                  Email…
                </button>
              </div>
            </>
          ) : (
            <div className="set-empty set-welcome">
              <span className="library-eyebrow">EVERY SET HAS A STORY</span>
              <h2>Your next set starts here.</h2>
              <p>
                Capture the tracks you play, add a location and notes,
                <br />
                then keep or share the finished tracklist.
              </p>
              <button
                className="set-primary"
                disabled={busy || !state || !!state.error}
                onClick={() => void start()}
              >
                ＋ Start set
              </button>
              <p className="set-hint">
                Or import histories from a USB library.
              </p>
            </div>
          )}
        </div>
      </div>
      <footer className="set-footer">
        <span>
          {state?.error
            ? "SAVE ERROR"
            : busy
              ? "SAVING…"
              : "LOCAL HOST STORAGE"}
        </span>
        <small>
          {state?.recording
            ? `Recording continues across screens · ${state.pending.map((p) => `CDJ${p.deck}: ${p.seconds}/45s`).join(" · ") || "Listening to CDJs"}`
            : state?.importNote ||
              "Original play events are preserved when you edit a tracklist."}
        </small>
      </footer>
    </section>
  );
}
