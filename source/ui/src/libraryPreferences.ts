export const builtInFilters = [
  { key: "rating", label: "Star rating" },
  { key: "bpm", label: "BPM range" },
  { key: "color", label: "Color" },
  { key: "key", label: "Musical key" },
  { key: "genre", label: "Track genre" },
  { key: "artist", label: "Artist" },
  { key: "label", label: "Record label" },
  { key: "format", label: "File format" },
  { key: "mytags", label: "All My Tags" },
  { key: "more", label: "Dates, year and duration" },
];
export const defaultLibraryFilters = [
  "rating",
  "bpm",
  "genre",
  "key",
  "color",
  "tag:*",
];
export function categoryKey(name: string) {
  return `tag:${encodeURIComponent(name)}`;
}
export function categoryName(key: string) {
  try {
    return decodeURIComponent(key.slice(4));
  } catch {
    return key.slice(4);
  }
}
export function parseLibraryFilters(value: unknown): string[] {
  if (!Array.isArray(value)) return [...defaultLibraryFilters];
  // Upgrade the former untouched default, while preserving customized orders.
  if (
    JSON.stringify(value) ===
    JSON.stringify(["rating", "bpm", "color", "key", "tag:*", "genre"])
  )
    return [...defaultLibraryFilters];
  return [
    ...new Set(
      value.filter(
        (v): v is string =>
          typeof v === "string" &&
          (builtInFilters.some((f) => f.key === v) || v.startsWith("tag:")),
      ),
    ),
  ].slice(0, 64);
}
export function displayedFilters(order: string[], categories: string[]) {
  return [
    ...new Set(
      order.flatMap((key) =>
        key === "tag:*"
          ? categories.length
            ? categories.map(categoryKey)
            : ["mytags"]
          : [key],
      ),
    ),
  ];
}
export function moveFilter(order: string[], key: string, direction: number) {
  const result = [...order],
    index = result.indexOf(key),
    next = index + direction;
  if (index < 0 || next < 0 || next >= result.length) return result;
  [result[index], result[next]] = [result[next], result[index]];
  return result;
}
export function nextColumnSort(current: Record<string, string>, field: string) {
  return {
    ...current,
    sort: field,
    direction:
      current.sort === field && current.direction !== "desc" ? "desc" : "asc",
  };
}

// Separate built-in metadata from personal My Tag categories, including names
// such as Genre that exist in both systems. Never change their query keys.
export function groupLibraryFilters(order: string[], categories: string[]) {
  const expanded = displayedFilters(order, categories);
  const personalized = expanded.filter(
    (key) => key === "mytags" || key.startsWith("tag:"),
  );
  const standard = expanded.filter((key) => !personalized.includes(key));
  return { standard, personalized };
}
