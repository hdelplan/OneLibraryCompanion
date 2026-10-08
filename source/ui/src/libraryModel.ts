export type LibrarySource = {
  direct?: boolean;
  id: string;
  label: string;
  available: boolean;
  loadable: boolean;
  loadUnavailableReason?: string | null;
  generation: number;
  state: string;
  error: string | null;
  count: number | null;
};
// Keep a deliberate choice while mounted; otherwise prefer the available USB.
export function selectedLibrarySource(
  current: string,
  sources: LibrarySource[],
): string {
  const available = sources.filter((source) => source.available);
  return (
    available.find((source) => source.id === current)?.id ??
    available.find((source) => source.direct || source.loadable)?.id ??
    available[0]?.id ??
    ""
  );
}
export type Playlist = {
  id: number;
  name: string;
  parentId: number;
  folder: boolean;
  order: number;
  count: number;
};
export type LibraryInfo = {
  fingerprint: string;
  generation: number;
  count: number;
  tagsAvailable: boolean;
  tagError: string | null;
  playlists: Playlist[];
  facets: Record<string, { value: string; count: number }[]>;
  categories: { id: number; name: string }[];
  tags: { id: number; categoryId: number; order: number; name: string }[];
};
export type LibraryTrack = {
  available?: boolean;
  savedArtwork?: string | null;
  id: number;
  entry: number;
  title: string;
  artist: string;
  album: string;
  genre: string;
  bpm: number;
  key: string;
  duration: number;
  rating: number;
  color: string;
  colorId: number;
  myTags: { id: number; name: string; category: string }[] | null;
  [field: string]: unknown;
};
export type TrackPage = {
  generation: number;
  total: number;
  offset: number;
  tracks: LibraryTrack[];
};
export type Filters = Record<string, string>;
export const emptyFilters: Filters = { sort: "playlist", tagMode: "any" };
export function clearTrackFilters(filters: Filters): Filters {
  return Object.fromEntries(
    Object.entries(filters).filter(([key]) =>
      ["playlist", "set", "sort", "direction", "tagMode"].includes(key),
    ),
  );
}
export function filterCount(filters: Filters) {
  return Object.entries(filters).filter(
    ([key, value]) =>
      value &&
      value !== "[]" &&
      !["playlist", "set", "sort", "direction", "tagMode"].includes(key) &&
      !key.endsWith(":mode"),
  ).length;
}
export function selection(filters: Filters, field: string): string[] {
  try {
    const v: unknown = JSON.parse(filters[field] ?? "[]");
    return Array.isArray(v)
      ? v.filter((x): x is string => typeof x === "string")
      : [];
  } catch {
    return [];
  }
}
export function folderPath(nodes: Playlist[], id: number): Playlist[] {
  const path: Playlist[] = [],
    seen = new Set<number>();
  let node = nodes.find((n) => n.id === id);
  while (node && !seen.has(node.id)) {
    seen.add(node.id);
    path.unshift(node);
    node = nodes.find((n) => n.id === node?.parentId);
  }
  return path;
}
export const colorPalette = [
  "#737373",
  "#f276b9",
  "#ea4248",
  "#f59b42",
  "#e8d44e",
  "#72c96b",
  "#61cbd0",
  "#558eef",
  "#aa79d7",
];
export function duration(seconds: number) {
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

// Omit cleared values, including those retained in older saved presets.
// [""] is intentionally retained: it selects missing metadata (e.g. No color).
export function activeFilterParams(filters: Filters): Filters {
  return Object.fromEntries(
    Object.entries(filters).filter(
      ([, value]) => value !== "" && value !== "[]",
    ),
  );
}

// Automatically select a local USB when it first becomes available. Repeated
// scans preserve the user's choice, including switching back to a CDJ USB.
export function newlyAvailableLocalSource(
  previous: LibrarySource[],
  next: LibrarySource[],
): string | undefined {
  return next.find(
    (s) =>
      s.available &&
      s.id.startsWith("local-usb:") &&
      !previous.some((p) => p.id === s.id && p.available),
  )?.id;
}
