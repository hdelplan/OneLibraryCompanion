import {
  builtInFilters,
  categoryKey,
  categoryName,
  displayedFilters,
  moveFilter,
} from "./libraryPreferences";
export function LibraryFilterConfiguration({
  order,
  bpmRange,
  changeBpmRange,
  categories,
  change,
}: {
  order: string[];
  bpmRange: { min: number; max: number };
  changeBpmRange: (range: { min: number; max: number }) => void;
  categories: string[];
  change: (value: string[]) => void;
}) {
  const visible = displayedFilters(order, categories);
  const options = [
    ...builtInFilters,
    ...categories.map((name) => ({
      key: categoryKey(name),
      label: `${name} · My Tag`,
    })),
  ];
  const keys = [
    ...visible,
    ...options.map((o) => o.key).filter((key) => !visible.includes(key)),
  ];
  return (
    <section className="settings-panel library-filter-config">
      <h2>Library filters</h2>
      <p>
        Choose which filters appear above the track list and move them into your
        preferred order. Album remains available in track details.
      </p>
      <div className="library-bpm-config">
        {(["min", "max"] as const).map((key) => (
          <label key={key}>
            {key === "min" ? "Minimum BPM option" : "Maximum BPM option"}
            <input
              type="number"
              min={1}
              max={999}
              step={1}
              value={bpmRange[key]}
              onChange={(e) => {
                const value = e.target.valueAsNumber;
                if (!Number.isInteger(value) || value < 1 || value > 999)
                  return;
                changeBpmRange(
                  key === "min"
                    ? { min: value, max: Math.max(value, bpmRange.max) }
                    : { min: Math.min(value, bpmRange.min), max: value },
                );
              }}
            />
          </label>
        ))}
      </div>
      {categories.length === 0 && (
        <p>
          Open a USB library with My Tags to discover its custom categories,
          such as Genre, Venue or Situation.
        </p>
      )}
      <div className="filter-config-list">
        {keys.map((key) => {
          const enabled = visible.includes(key);
          return (
            <div className="filter-config-row" key={key}>
              <label>
                <input
                  type="checkbox"
                  checked={enabled}
                  onChange={(e) =>
                    change(
                      e.target.checked
                        ? [...visible, key]
                        : visible.filter((v) => v !== key),
                    )
                  }
                />
                {options.find((o) => o.key === key)?.label ??
                  `${categoryName(key)} · My Tag (not on current USB)`}
              </label>
              <button
                aria-label={`Move ${options.find((o) => o.key === key)?.label ?? categoryName(key)} up`}
                disabled={!enabled || visible.indexOf(key) === 0}
                onClick={() => change(moveFilter(visible, key, -1))}
              >
                ↑
              </button>
              <button
                aria-label={`Move ${options.find((o) => o.key === key)?.label ?? categoryName(key)} down`}
                disabled={
                  !enabled || visible.indexOf(key) === visible.length - 1
                }
                onClick={() => change(moveFilter(visible, key, 1))}
              >
                ↓
              </button>
            </div>
          );
        })}
      </div>
      <label className="filter-auto-tags">
        <input
          type="checkbox"
          checked={order.includes("tag:*")}
          onChange={(e) =>
            change(
              e.target.checked
                ? [...order.filter((key) => !key.startsWith("tag:")), "tag:*"]
                : visible,
            )
          }
        />
        Automatically include all exported My Tag categories
      </label>
      <p>
        Custom category names and values come from the USB. Preferences are
        saved on this device; they do not edit Rekordbox.
      </p>
    </section>
  );
}
