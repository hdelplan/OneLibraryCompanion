import { useState } from "react";
export function Facet({
  field,
  values,
  selected,
  change,
}: {
  field: string;
  values: { value: string; count: number }[];
  selected: string[];
  change: (values: string[]) => void;
}) {
  const [search, setSearch] = useState("");
  const matches = values.filter((v) =>
    v.value.toLowerCase().includes(search.toLowerCase()),
  );
  return (
    <details className="library-facet">
      <summary>
        {field[0].toUpperCase() + field.slice(1)}
        {selected.length ? ` · ${selected.length}` : ""}
      </summary>
      <div className="library-options">
        <input
          type="search"
          aria-label={`Find ${field}`}
          placeholder={`Find ${field}…`}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        {matches.slice(0, 100).map((f) => (
          <label key={f.value}>
            <input
              type="checkbox"
              checked={selected.includes(f.value)}
              onChange={(e) =>
                change(
                  e.target.checked
                    ? [...selected, f.value]
                    : selected.filter((v) => v !== f.value),
                )
              }
            />
            {f.value || (field === "color" ? "No color" : "Not set")}
            <small>{f.count}</small>
          </label>
        ))}
        {matches.length > 100 && (
          <p>Showing 100 values. Type to narrow the list.</p>
        )}
        <small>Counts across this USB</small>
      </div>
    </details>
  );
}
