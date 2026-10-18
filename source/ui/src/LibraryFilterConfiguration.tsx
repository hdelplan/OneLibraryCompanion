import {
  builtInFilters,
  categoryKey,
  categoryName,
  groupLibraryFilters,
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
  const groups = groupLibraryFilters(order, categories);
  const visible = [...groups.standard, ...groups.personalized];
  const options = [
    ...builtInFilters,
    ...categories.map((name) => ({ key: categoryKey(name), label: name })),
  ];
  function update(
    group: "standard" | "personalized",
    next: string[],
    keepAuto = true,
  ) {
    const combined =
      group === "standard"
        ? [...next, ...groups.personalized]
        : [...groups.standard, ...next];
    change([
      ...combined,
      ...(keepAuto && order.includes("tag:*") ? ["tag:*"] : []),
    ]);
  }
  function renderGroup(
    group: "standard" | "personalized",
    title: string,
    description: string,
  ) {
    const enabledKeys = groups[group];
    const available = options.filter((option) =>
      group === "personalized"
        ? option.key === "mytags" || option.key.startsWith("tag:")
        : option.key !== "mytags" && !option.key.startsWith("tag:"),
    );
    const keys = [
      ...enabledKeys,
      ...available
        .map((option) => option.key)
        .filter((key) => !enabledKeys.includes(key)),
    ];
    return (
      <section className="filter-config-group" aria-label={title}>
        <h3>{title}</h3>
        <p>{description}</p>
        <div className="filter-config-list">
          {keys.map((key) => {
            const enabled = enabledKeys.includes(key);
            const name =
              options.find((option) => option.key === key)?.label ??
              `${categoryName(key)} (not on current USB)`;
            return (
              <div className="filter-config-row" key={key}>
                <label>
                  <input
                    type="checkbox"
                    checked={enabled}
                    onChange={(e) =>
                      update(
                        group,
                        e.target.checked
                          ? [...enabledKeys, key]
                          : enabledKeys.filter((value) => value !== key),
                        group === "standard" ||
                          e.target.checked ||
                          key === "mytags",
                      )
                    }
                  />
                  {name}
                </label>
                <button
                  aria-label={`Move ${name} up`}
                  disabled={!enabled || enabledKeys.indexOf(key) === 0}
                  onClick={() =>
                    update(group, moveFilter(enabledKeys, key, -1))
                  }
                >
                  ↑
                </button>
                <button
                  aria-label={`Move ${name} down`}
                  disabled={
                    !enabled ||
                    enabledKeys.indexOf(key) === enabledKeys.length - 1
                  }
                  onClick={() => update(group, moveFilter(enabledKeys, key, 1))}
                >
                  ↓
                </button>
              </div>
            );
          })}
        </div>
        {group === "personalized" && (
          <>
            {categories.length === 0 && (
              <p>
                Open a USB library with My Tags to discover its custom
                categories.
              </p>
            )}
            <label className="filter-auto-tags">
              <input
                type="checkbox"
                checked={order.includes("tag:*")}
                onChange={(e) =>
                  change(e.target.checked ? [...visible, "tag:*"] : visible)
                }
              />
              Automatically include all exported My Tag categories
            </label>
          </>
        )}
      </section>
    );
  }
  return (
    <section className="settings-panel library-filter-config">
      <h2>Library filters</h2>
      <p>
        Choose which filters appear above the track list and reorder them within
        each section. Changes apply immediately to Browse.
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
      <div className="filter-config-groups">
        {renderGroup(
          "standard",
          "Standard metadata",
          "Choose and reorder track metadata filters. Color starts last by default; move it anywhere within this section.",
        )}
        {renderGroup(
          "personalized",
          "My Tags",
          "Choose and reorder your personalized Rekordbox tag categories. These appear together after standard metadata.",
        )}
      </div>
      <p>
        Custom category names and values come from the USB. Preferences are
        saved on this device; they do not edit Rekordbox.
      </p>
    </section>
  );
}
