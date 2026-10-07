export type Column = { low: number; mid: number; high: number };
export type Wave = {
  tag: string;
  columns: Column[];
  rawColumns: number[][];
  samplesPerSecond: number | null;
  normalization: number;
};
export type TrackInfo = {
  id: number;
  title: string;
  artist: string;
  album: string;
  key: string;
  bpm: number;
  duration: number;
  genre: string;
  rating?: number;
  color?: string;
  comment?: string;
  dateAdded?: string;
  myTags?: { name: string; category?: string | null }[] | null;
  bitrate: number;
  sampleRate: number;
};
export type Analysis = {
  beats?: { time: number; beatInBar: number }[];
  artworkAvailable?: boolean;
  artworkUrl?: string;
  cues?: { time: number; hot: number; label: string; color: string }[];
  phraseMood?: "Low" | "Mid" | "High" | null;
  phraseWarning?: string | null;
  phrases?: {
    start: number;
    end: number;
    label: string;
    kind: number;
    color?: string;
    textColor?: string;
  }[];
  detail: Wave | null;
  preview: Wave | null;
  track?: TrackInfo | null;
};
export type Deck = {
  name: string;
  source: "offline" | "live";
  analysis: Analysis;
  position: number | null;
  live?: LivePlayer;
  motion?: import("./playbackClock").Motion;
} | null;
export type LivePlayer = {
  loadProtected?: boolean;
  playing?: boolean;
  manualMotion?: boolean;
  currentCue?: number | null;
  loop?: {
    start: number;
    end: number;
    beats: number;
    estimated: boolean;
  } | null;
  beatNumber?: number | null;
  reverse?: boolean;
  statusAgeMs?: number | null;
  positionAgeMs?: number | null;
  motionRate?: number | null;
  beatAnchorNumber?: number | null;
  observationId?: string | null;
  observationTimeMs?: number | null;
  packetCounter?: number | null;
  positionQuality?: "beat" | "fine" | "held" | "coarse" | "unavailable";
  positionSource?: string;
  number: number;
  name: string;
  ip: string;
  connection: "connected" | "stale" | "disconnected";
  playState: string | null;
  bpm: number | null;
  pitch: number | null;
  master: boolean | null;
  sync: boolean | null;
  trackKey: string | null;
  assetReady: boolean;
  position: number | null;
  warning: string | null;
  sourceLabel: string | null;
};
export function durationOf(deck: Deck): number | null {
  const detail = deck?.analysis.detail;
  return detail
    ? detail.columns.length / (detail.samplesPerSecond ?? 150)
    : (deck?.analysis.track?.duration ?? null);
}
