import { useEffect, useState } from "react";
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
const textFields = [
  "title",
  "artist",
  "album",
  "genre",
  "key",
  "label",
  "composer",
  "originalArtist",
  "remixer",
  "color",
  "comment",
  "isrc",
  "dateAdded",
  "releaseDate",
  "mixName",
  "filePath",
  "filename",
  "analysisPath",
  "artworkPath",
  "format",
];
const numberFields = [
  "id",
  "bpm",
  "duration",
  "bitrate",
  "sampleRate",
  "sampleDepth",
  "fileSize",
  "trackNumber",
  "discNumber",
  "year",
  "rating",
  "playCount",
  "colorId",
];
type Rule = { field: string; op: string; value: string };
function readRules(value: string): Rule[] {
  try {
    const parsed: unknown = JSON.parse(value);
    return Array.isArray(parsed)
      ? parsed.filter(
          (r): r is Rule =>
            typeof r?.field === "string" &&
            typeof r?.op === "string" &&
            typeof r?.value === "string",
        )
      : [];
  } catch {
    return [];
  }
}
export function AdvancedFilters({
  value,
  change,
}: {
  value: string;
  change: (value: string) => void;
}) {
  const [rules, setRules] = useState<Rule[]>(() => readRules(value));
  useEffect(() => setRules(readRules(value)), [value]);
  function update(index: number, patch: Partial<Rule>) {
    setRules((rows) =>
      rows.map((r, i) => (i === index ? { ...r, ...patch } : r)),
    );
  }
  return (
    <details className="library-advanced">
      <summary>
        Advanced metadata rules{value && value !== "[]" ? " · active" : ""}
      </summary>
      <div className="library-rule-editor">
        <p>
          Match every rule. Dates use YYYY-MM-DD; duration is seconds and file
          size is bytes.
        </p>
        {rules.map((rule, index) => (
          <div className="library-rule" key={index}>
            <select
              aria-label={`Rule ${index + 1} field`}
              value={rule.field}
              onChange={(e) =>
                update(index, {
                  field: e.target.value,
                  op: numberFields.includes(e.target.value) ? "eq" : "contains",
                  value: "",
                })
              }
            >
              {[...textFields, ...numberFields].map((field) => (
                <option key={field} value={field}>
                  {field.replace(/([A-Z])/g, " $1")}
                </option>
              ))}
            </select>
            <select
              aria-label={`Rule ${index + 1} operator`}
              value={rule.op}
              onChange={(e) => update(index, { op: e.target.value })}
            >
              {(numberFields.includes(rule.field)
                ? [
                    ["eq", "equals"],
                    ["gte", "at least"],
                    ["lte", "at most"],
                  ]
                : [
                    ["contains", "contains"],
                    ["notContains", "does not contain"],
                    ["eq", "equals"],
                    ["gte", "on or after"],
                    ["lte", "on or before"],
                    ["empty", "is empty"],
                    ["notEmpty", "is set"],
                  ]
              ).map(([op, label]) => (
                <option key={op} value={op}>
                  {label}
                </option>
              ))}
            </select>
            <input
              aria-label={`Rule ${index + 1} value`}
              type={numberFields.includes(rule.field) ? "number" : "text"}
              step="any"
              disabled={["empty", "notEmpty"].includes(rule.op)}
              value={rule.value}
              onChange={(e) => update(index, { value: e.target.value })}
            />
            <button
              aria-label={`Remove rule ${index + 1}`}
              onClick={() => setRules((r) => r.filter((_, i) => i !== index))}
            >
              ×
            </button>
          </div>
        ))}
        <div className="library-rule-actions">
          <button
            disabled={rules.length >= 16}
            onClick={() =>
              setRules((r) => [
                ...r,
                { field: "comment", op: "contains", value: "" },
              ])
            }
          >
            + Add rule
          </button>
          <button
            onClick={() => change(rules.length ? JSON.stringify(rules) : "")}
          >
            Apply rules
          </button>
          <button
            onClick={() => {
              setRules([]);
              change("");
            }}
          >
            Clear rules
          </button>
        </div>
      </div>
    </details>
  );
}
