import { densityRows, type LibraryDensity } from "./libraryDensity";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Facet } from "./LibraryFilters";
import { groupLibraryFilters, categoryName } from "./libraryPreferences";
import { selection, type Filters, type LibraryInfo } from "./libraryModel";
function CategoryFilter({
  name,
  info,
  filters,
  change,
  inline = false,
}: {
  inline?: boolean;
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
  if (inline)
    return (
      <div
        className="library-inline-tags"
        role="group"
        aria-label={`${name} direct filters`}
      >
        {tags.map((tag) => (
          <button
            type="button"
            key={tag.id}
            aria-pressed={chosen.includes(String(tag.id))}
            onClick={() =>
              change(
                key,
                JSON.stringify(
                  chosen.includes(String(tag.id))
                    ? chosen.filter((id) => id !== String(tag.id))
                    : [...chosen, String(tag.id)],
                ),
              )
            }
          >
            {tag.name}
          </button>
        ))}
        <select
          aria-label={`${name} matching`}
          value={filters[`${key}:mode`] ?? "any"}
          onChange={(e) => change(`${key}:mode`, e.target.value)}
        >
          <option value="any">ANY</option>
          <option value="all">ALL</option>
          <option value="none">NONE</option>
        </select>
      </div>
    );
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
  const overflowButton = useRef<HTMLButtonElement>(null);
  const [inlineOther, setInlineOther] = useState(false);
  const [visibleCount, setVisibleCount] = useState(Infinity);
  const [overflowOpen, setOverflowOpen] = useState(false);
  useEffect(() => {
    function closeOutside(event: PointerEvent | FocusEvent) {
      if (!(event.target instanceof Node)) return;
      if (!(event.target as Element).closest?.(".library-filter-overflow"))
        setOverflowOpen(false);
      root.current
        ?.querySelectorAll<HTMLDetailsElement>("details[open]")
        .forEach((menu) => {
          if (!menu.contains(event.target as Node)) menu.open = false;
        });
    }
    function escape(event: KeyboardEvent) {
      if (event.key === "Escape") {
        if (
          root.current
            ?.querySelector(".library-filter-overflow")
            ?.contains(document.activeElement)
        )
          overflowButton.current?.focus();
        setOverflowOpen(false);
        root.current
          ?.querySelectorAll<HTMLDetailsElement>("details[open]")
          .forEach((menu) => {
            menu.open = false;
            if (!menu.closest(".library-filter-overflow"))
              menu.querySelector("summary")?.focus();
          });
      }
    }
    const element = root.current;
    element?.addEventListener("toggle", placeMenu, true);
    document.addEventListener("pointerdown", closeOutside);
    document.addEventListener("focusin", closeOutside);
    document.addEventListener("keydown", escape);
    return () => {
      element?.removeEventListener("toggle", placeMenu, true);
      document.removeEventListener("pointerdown", closeOutside);
      document.removeEventListener("focusin", closeOutside);
      document.removeEventListener("keydown", escape);
    };
  }, []);
  const bpms = Array.from(
    { length: bpmRange.max - bpmRange.min + 1 },
    (_, i) => bpmRange.min + i,
  );
  const groups = groupLibraryFilters(
    order,
    info?.categories.map((c) => c.name) ?? [],
  );
  const keys = [...groups.standard, ...groups.personalized];
  const otherField = groups.personalized.find(
    (field) =>
      field.startsWith("tag:") &&
      categoryName(field).toLowerCase() === "other tags",
  );
  const renderFilter = (field: string) => {
    if (field === otherField)
      return (
        <div className="library-other-tags" data-inline={inlineOther}>
          <div className="library-other-compact">
            <CategoryFilter
              name={categoryName(field)}
              info={info}
              filters={filters}
              change={change}
            />
          </div>
          <div className="library-other-direct">
            <CategoryFilter
              inline
              name={categoryName(field)}
              info={info}
              filters={filters}
              change={change}
            />
          </div>
        </div>
      );
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
                                  : ids.filter((id) => id !== String(tag.id))
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
  };
  const keySignature = keys.join("\n");
  useLayoutEffect(() => {
    const element = root.current;
    if (!element) return;
    function fit() {
      if (!element || element.clientWidth === 0) return;
      const density = element.querySelector<HTMLElement>(".library-density");
      const gap = parseFloat(getComputedStyle(element).columnGap) || 0;
      const section = element.querySelector<HTMLElement>(
        ".library-filter-section",
      );
      const itemGap = section
        ? parseFloat(getComputedStyle(section).columnGap) || 4
        : 4;
      // Overflow controls remain measurable while their panel is invisible.
      const slots = Array.from(
        element.querySelectorAll<HTMLElement>(".library-filter-slot"),
      ).sort(
        (a, b) => Number(a.dataset.filterIndex) - Number(b.dataset.filterIndex),
      );
      const widths = slots.map(
        (slot) =>
          slot.querySelector<HTMLElement>(".library-other-compact")
            ?.offsetWidth ?? slot.offsetWidth,
      );
      const available =
        element.clientWidth - (density?.offsetWidth ?? 118) - gap;
      const standardCount = groups.standard.length;
      const sectionGap = 10;
      function occupied(count: number) {
        const standard = Math.min(count, standardCount);
        const personal = Math.max(0, count - standardCount);
        const standardWidth =
          widths.slice(0, standard).reduce((sum, width) => sum + width, 0) +
          Math.max(0, standard - 1) * itemGap;
        const personalWidth = personal
          ? Math.max(
              personal >= 3 ? 150 : 45,
              widths
                .slice(standardCount, count)
                .reduce((sum, width) => sum + width, 0) +
                Math.max(0, personal - 1) * itemGap,
            ) + 10
          : 0;
        return (
          standardWidth +
          personalWidth +
          (standard && personal ? sectionGap : 0)
        );
      }
      let count = widths.length;
      if (occupied(count) > available) {
        const budget = available - 98 - sectionGap;
        count = 0;
        while (count < widths.length && occupied(count + 1) <= budget) count++;
      }
      const direct = element.querySelector<HTMLElement>(
        ".library-other-direct",
      );
      const compact = element.querySelector<HTMLElement>(
        ".library-other-compact",
      );
      setInlineOther(
        Boolean(
          direct &&
            compact &&
            count === widths.length &&
            occupied(count) + direct.offsetWidth - compact.offsetWidth <=
              available - 4,
        ),
      );
      setVisibleCount((previous) => (previous === count ? previous : count));
    }
    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(element);
    return () => observer.disconnect();
  }, [keySignature, filters, info]);
  useEffect(() => {
    if (!overflowOpen)
      root.current
        ?.querySelectorAll<HTMLDetailsElement>(
          ".library-filter-overflow details[open]",
        )
        .forEach((menu) => {
          menu.open = false;
        });
  }, [overflowOpen]);
  const shown = Math.min(visibleCount, keys.length);
  function filterSlot(field: string, index: number) {
    return (
      <div
        className="library-filter-slot"
        data-filter-index={index}
        data-filter={field}
        data-personalized={groups.personalized.includes(field)}
        key={field}
      >
        {field === "genre" && (
          <span className="library-standard-filter-label">Track genre</span>
        )}
        {renderFilter(field)}
      </div>
    );
  }
  function renderSections(fields: string[], offset: number) {
    const standard = fields.filter((field) => groups.standard.includes(field));
    const personal = fields.filter((field) =>
      groups.personalized.includes(field),
    );
    return (
      <>
        {standard.length > 0 && (
          <div
            className="library-filter-section"
            role="group"
            aria-label="Standard track filters"
          >
            {standard.map((field) =>
              filterSlot(field, offset + fields.indexOf(field)),
            )}
          </div>
        )}
        {personal.length > 0 && (
          <div
            className="library-filter-section library-personal-filters"
            role="group"
            aria-label="My Tags personalized filters"
          >
            <div
              className="library-personal-helper"
              data-compact={personal.length < 3}
            >
              <svg viewBox="0 0 16 16" aria-hidden="true">
                <path d="M2 2h5l7 7-5 5-7-7z" />
                <circle cx="5" cy="5" r="1" />
              </svg>
              <strong>MY TAGS</strong>
              <span>Your personalized filters</span>
            </div>
            {personal.map((field) =>
              filterSlot(field, offset + fields.indexOf(field)),
            )}
          </div>
        )}
      </>
    );
  }
  function placeMenu(event: Event) {
    const menu = event.target;
    if (!(menu instanceof HTMLDetailsElement) || !menu.open) return;
    const popup = menu.querySelector<HTMLElement>(".library-options");
    const content = root.current?.closest(".library-content");
    if (!popup || !content) return;
    popup.style.left = "0px";
    const bounds = content.getBoundingClientRect();
    const box = popup.getBoundingClientRect();
    const scale = menu.getBoundingClientRect().width / menu.offsetWidth || 1;
    const shift = Math.max(
      bounds.left + 8 - box.left,
      Math.min(0, bounds.right - 8 - box.right),
    );
    popup.style.left = `${shift / scale}px`;
  }
  return (
    <div className="library-filters" ref={root}>
      <div className="library-filter-controls">
        {renderSections(keys.slice(0, shown), 0)}
        <div className="library-filter-overflow">
          <button
            type="button"
            className="library-filter-overflow-button"
            ref={overflowButton}
            hidden={shown === keys.length}
            aria-expanded={overflowOpen && shown < keys.length}
            aria-controls="library-additional-filters"
            onClick={() => setOverflowOpen((open) => !open)}
          >
            More filters <span aria-hidden="true">⌄</span>
          </button>
          <div
            id="library-additional-filters"
            className="library-filter-overflow-panel"
            data-open={overflowOpen && shown < keys.length}
            role="group"
            aria-label="Additional library filters"
          >
            {renderSections(keys.slice(shown), shown)}
          </div>
        </div>
      </div>

      <div
        className="library-density"
        role="group"
        aria-label="Track text size"
      >
        <small className="library-density-label">Text size</small>
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
