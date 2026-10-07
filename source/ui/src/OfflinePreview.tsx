export type OfflinePreviewProps = {
  previewDeck: number;
  setPreviewDeck: (deck: number) => void;
  busy: boolean;
  load: (file?: File) => Promise<void>;
  error: string;
};
export function OfflinePreview({
  previewDeck,
  setPreviewDeck,
  busy,
  load,
  error,
}: OfflinePreviewProps) {
  return (
    <section className="settings-panel offline-panel">
      <h2>Offline preview</h2>
      <p>
        Inspect a saved analysis while the CDJs are unavailable. This does not
        load a track onto a CDJ.
      </p>
      <div className="offline-controls">
        <label>
          Preview deck
          <select
            value={previewDeck}
            onChange={(e) => setPreviewDeck(+e.target.value)}
          >
            <option value={0}>Deck 1</option>
            <option value={1}>Deck 2</option>
          </select>
        </label>
        <button className="primary" disabled={busy} onClick={() => load()}>
          Open saved capture
        </button>
        <label className="file-button">
          Open analysis file
          <input
            aria-label="Open analysis file"
            type="file"
            accept=".2ex,.2EX,.EXT,.DAT"
            disabled={busy}
            onChange={(e) => {
              const f = e.target.files?.[0];
              if (f) void load(f);
              e.target.value = "";
            }}
          />
        </label>
      </div>
      {busy && <p role="status">Reading analysis…</p>}
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
