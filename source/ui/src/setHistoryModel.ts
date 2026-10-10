export type SetTrack = {
  filePath?: string;
  id: number;
  title: string;
  artist: string;
  key: string;
  rating: number;
  artwork: string | null;
};
export type SetEvent = {
  playedAt?: number | null;
  id: string;
  deck: number | null;
  source: string;
  track: SetTrack;
};
export type DJSet = {
  id: string;
  title: string;
  startedAt: number;
  endedAt: number | null;
  dateOnly: boolean;
  location: string;
  comment: string;
  origin: "recorded" | "automatic" | "recovered" | "imported" | "sample";
  recovered: boolean;
  events: SetEvent[];
  order: string[];
};
export type SetHistoryState = {
  version: number;
  revision: number;
  sets: DJSet[];
  activeId: string | null;
  recording: boolean;
  observationInterrupted?: boolean;
  error: string | null;
  importNote: string;
  playedTracks?: SetTrack[];
  pending: { deck: number; seconds: number }[];
};
export type ExportFormat = "txt" | "csv" | "pdf";
export function orderedTracks(set: DJSet): SetEvent[] {
  const events = new Map(set.events.map((event) => [event.id, event]));
  return set.order.flatMap((id) => (events.has(id) ? [events.get(id)!] : []));
}
export function setDate(set: DJSet, short = false) {
  return new Date(set.startedAt).toLocaleString(
    "en-GB",
    set.dateOnly
      ? { year: "numeric", month: "short", day: "numeric", timeZone: "UTC" }
      : {
          year: short ? undefined : "numeric",
          month: "short",
          day: "numeric",
          hour: "2-digit",
          minute: "2-digit",
        },
  );
}
export function setText(sets: DJSet[]): string {
  return (
    sets
      .map((set) =>
        [
          `SET HISTORY — ${set.title}`,
          `${setDate(set)}${set.dateOnly ? " (time unavailable)" : ""}`,
          `Source: ${set.origin === "sample" ? "SAMPLE — not a recorded performance" : set.origin}`,
          `Location: ${set.location || "Not specified"}`,
          `Comment: ${set.comment || "—"}`,
          "",
          ...orderedTracks(set).map(
            ({ track }, i) =>
              `${i + 1}. ${track.title} — ${track.artist || "Unknown artist"} | ${track.key || "No key"} | ${track.rating}/5 stars`,
          ),
        ].join("\n"),
      )
      .join("\n\n" + "─".repeat(60) + "\n\n") + "\n"
  );
}
// Quote multiline/Unicode fields, and neutralise spreadsheet formula prefixes.
function csvCell(value: unknown): string {
  let text = String(value ?? "");
  if (/^[\s]*[=+@-]/.test(text) || /^[\t\r\n]/.test(text)) text = "'" + text;
  return `"${text.replaceAll('"', '""')}"`;
}
export function setCsv(sets: DJSet[]): string {
  const rows: unknown[][] = [
    [
      "Set",
      "Date",
      "Location",
      "Comment",
      "Source",
      "Sequence",
      "Title",
      "Artist",
      "Key",
      "Rating",
    ],
  ];
  for (const set of sets) {
    const prefix = [
      set.title,
      set.dateOnly
        ? new Date(set.startedAt).toISOString().slice(0, 10)
        : new Date(set.startedAt).toISOString(),
      set.location,
      set.comment,
      set.origin,
    ];
    const tracks = orderedTracks(set);
    if (!tracks.length) rows.push([...prefix, "", "", "", "", ""]);
    tracks.forEach(({ track }, i) =>
      rows.push([
        ...prefix,
        i + 1,
        track.title,
        track.artist,
        track.key,
        track.rating,
      ]),
    );
  }
  return (
    "\uFEFF" +
    rows.map((row) => row.map(csvCell).join(",")).join("\r\n") +
    "\r\n"
  );
}

// Paths plus catalog ID and metadata distinguish remixes and reused numeric IDs.
export function playedTrackKey(track: {
  id: number;
  title: string;
  artist: string;
  filePath?: unknown;
}): string | null {
  return typeof track.filePath === "string" && track.filePath.length > 0
    ? JSON.stringify([track.filePath, track.id, track.title, track.artist])
    : null;
}
export function playedTrackKeys(history: SetHistoryState | null): Set<string> {
  return new Set(
    (history?.playedTracks ?? []).flatMap((track) => {
      const key = playedTrackKey(track);
      return key ? [key] : [];
    }),
  );
}
