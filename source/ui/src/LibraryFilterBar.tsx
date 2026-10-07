import { densityRows, type LibraryDensity } from "./libraryDensity";
import { useEffect, useRef } from "react";
import { Facet } from "./LibraryFilters";
import { displayedFilters, categoryName } from "./libraryPreferences";
import { selection, type Filters, type LibraryInfo } from "./libraryModel";
function CategoryFilter({
  name,
  info,
  filters,
  change,
}: {
  name: string;
  info: LibraryInfo | null;
  filters: Filters;
  change: (key: string, value: string) => void;
}) {
  const category = info?.categories.find((c) => c.name === name);
  const key = category ? `tagCategory:${category.id}` : "";
  const chosen = selection(filters, key);
  const tags =
    info?.tags
      .filter((t) => t.categoryId === category?.id)
      .sort((a, b) => a.order - b.order) ?? [];
  return (
    <details className="library-facet">
      <summary>
        {name}
        {chosen.length ? ` · ${chosen.length}` : ""}
      </summary>
      <div className="library-options library-tags">
        <small>MY TAG CATEGORY</small>
        {!category ? (
          <p>No values for this category.</p>
        ) : (
          <>
            <select
              aria-label={`${name} matching`}
              value={filters[`${key}:mode`] ?? "any"}
              onChange={(e) => change(`${key}:mode`, e.target.value)}
            >
              <option value="any">ANY selected value</option>
              <option value="all">ALL selected values</option>
              <option value="none">NONE of these values</option>
            </select>
            {tags.map((tag) => (
              <label key={tag.id}>
                <input
                  type="checkbox"
                  checked={chosen.includes(String(tag.id))}
                  onChange={(e) =>
                    change(
                      key,
                      JSON.stringify(
                        e.target.checked
                          ? [...chosen, String(tag.id)]
                          : chosen.filter((id) => id !== String(tag.id)),
                      ),
                    )
                  }
                />
                {tag.name}
              </label>
            ))}
          </>
        )}
      </div>
    </details>
  );
}
export function LibraryFilterBar({
  density,
  onDensityChange,
  order,
  bpmRange,
  info,
  filters,
  change,
}: {
  density: LibraryDensity;
  onDensityChange: (density: LibraryDensity) => void;
  order: string[];
  bpmRange: { min: number; max: number };
  info: LibraryInfo | null;
  filters: Filters;
  change: (key: string, value: string) => void;
}) {
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    function closeOutside(event: PointerEvent | FocusEvent) {
      if (!(event.target instanceof Node)) return;
      root.current
        ?.querySelectorAll<HTMLDetailsElement>("details[open]")
        .forEach((menu) => {
          if (!menu.contains(event.target as Node)) menu.open = false;
        });
    }
    function escape(event: KeyboardEvent) {
      if (event.key === "Escape")
        root.current
          ?.querySelectorAll<HTMLDetailsElement>("details[open]")
          .forEach((menu) => {
            menu.open = false;
            menu.querySelector("summary")?.focus();
          });
    }
    document.addEventListener("pointerdown", closeOutside);
    document.addEventListener("focusin", closeOutside);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("pointerdown", closeOutside);
      document.removeEventListener("focusin", closeOutside);
      document.removeEventListener("keydown", escape);
    };
  }, []);
  const bpms = Array.from(
    { length: bpmRange.max - bpmRange.min + 1 },
    (_, i) => bpmRange.min + i,
  );
  const keys = displayedFilters(
    order,
    info?.categories.map((c) => c.name) ?? [],
  );
  return (
    <div className="library-filters" ref={root}>
      {keys.map((field) => {
        if (field.startsWith("tag:"))
          return (
            <CategoryFilter
              key={field}
              name={categoryName(field)}
              info={info}
              filters={filters}
              change={change}
            />
          );
        if (field === "rating")
          return (
            <label key={field}>
              Rating
              <select
                value={filters.rating ?? ""}
                onChange={(e) => change("rating", e.target.value)}
              >
                <option value="">Any rating</option>
                <option value="unrated">Unrated</option>
                {[1, 2, 3, 4, 5].map((n) => (
                  <option key={n} value={n}>
                    {"★".repeat(n)}
                    {n < 5 ? " & up" : ""}
                  </option>
                ))}
              </select>
            </label>
          );
        if (field === "bpm")
          return (
            <div className="library-bpm" key={field}>
              {[
                ["bpmMin", "BPM from"],
                ["bpmMax", "BPM to"],
              ].map(([key, label]) => (
                <label key={key}>
                  {label}
                  <select
                    value={filters[key] ?? ""}
                    onChange={(e) => change(key, e.target.value)}
                  >
                    <option value="">Any</option>
                    {bpms.map((bpm) => (
                      <option key={bpm} value={bpm}>
                        {bpm}
                      </option>
                    ))}
                  </select>
                </label>
              ))}
            </div>
          );
        if (field === "more")
          return (
            <details className="library-facet" key={field}>
              <summary>More</summary>
              <div className="library-options">
                {[
                  ["yearMin", "Year from"],
                  ["yearMax", "Year to"],
                  ["durationMin", "Duration from (sec)"],
                  ["durationMax", "Duration to (sec)"],
                  ["addedFrom", "Added after"],
                  ["addedTo", "Added before"],
                ].map(([key, label]) => (
                  <label key={key}>
                    {label}
                    <input
                      type={key.startsWith("added") ? "date" : "number"}
                      min="0"
                      value={filters[key] ?? ""}
                      onChange={(e) => change(key, e.target.value)}
                    />
                  </label>
                ))}
              </div>
            </details>
          );
        if (field === "mytags")
          return (
            <details className="library-facet" key={field}>
              <summary>
                My Tags
                {filters.tags ? ` · ${filters.tags.split(",").length}` : ""}
              </summary>
              <div className="library-options library-tags">
                {!info?.tagsAvailable ? (
                  <p>No My Tags to display.</p>
                ) : (
                  <>
                    <select
                      aria-label="My Tag matching"
                      value={filters.tagMode}
                      onChange={(e) => change("tagMode", e.target.value)}
                    >
                      <option value="any">Match ANY selected tag</option>
                      <option value="all">Match ALL selected tags</option>
                      <option value="none">Match NONE of these tags</option>
                    </select>
                    {info.categories.map((category) => (
                      <div key={category.id}>
                        <strong>{category.name}</strong>
                        {info.tags
                          .filter((tag) => tag.categoryId === category.id)
                          .sort((a, b) => a.order - b.order)
                          .map((tag) => (
                            <label key={tag.id}>
                              <input
                                type="checkbox"
                                checked={(filters.tags ?? "")
                                  .split(",")
                                  .includes(String(tag.id))}
                                onChange={(e) => {
                                  const ids = (filters.tags ?? "")
                                    .split(",")
                                    .filter(Boolean);
                                  change(
                                    "tags",
                                    (e.target.checked
                                      ? [...ids, String(tag.id)]
                                      : ids.filter(
                                          (id) => id !== String(tag.id),
                                        )
                                    ).join(","),
                                  );
                                }}
                              />
                              {tag.name}
                            </label>
                          ))}
                      </div>
                    ))}
                  </>
                )}
              </div>
            </details>
          );
        return (
          <Facet
            key={field}
            field={field}
            values={info?.facets[field] ?? []}
            selected={selection(filters, field)}
            change={(values) => change(field, JSON.stringify(values))}
          />
        );
      })}
      <div
        className="library-density"
        role="group"
        aria-label="Track text size"
      >
        {(["small", "medium", "large"] as const).map((value) => (
          <button
            key={value}
            type="button"
            data-density={value}
            aria-label={`${value[0].toUpperCase() + value.slice(1)} text — ${densityRows[value]} tracks`}
            title={`${value[0].toUpperCase() + value.slice(1)} text — ${densityRows[value]} tracks`}
            aria-pressed={density === value}
            onClick={() => onDensityChange(value)}
          >
            A
          </button>
        ))}
      </div>
    </div>
  );
}
