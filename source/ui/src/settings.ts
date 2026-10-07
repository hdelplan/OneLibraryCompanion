import {
  defaultLibraryFilters,
  parseLibraryFilters,
} from "./libraryPreferences";
export type Settings = {
  libraryFilters: string[];
  libraryBpmRange: { min: number; max: number };
  jogSmoothing: number;
  playhead: "third" | "center";
  playheadColor: "red" | "white";
  window: number;
  bass: "balanced" | "emphasis";
  time: "remaining" | "elapsed";
  overview: boolean;
  phrases: boolean;
  details: boolean;
};
export const defaults: Settings = {
  libraryFilters: [...defaultLibraryFilters],
  libraryBpmRange: { min: 115, max: 130 },
  jogSmoothing: 0,
  playhead: "third",
  playheadColor: "white",
  window: 8,
  bass: "emphasis",
  time: "remaining",
  overview: true,
  phrases: true,
  details: true,
};
export const settingsKey = "pioneercompanion.display.v1";
export function parseSettings(raw: string | null): Settings {
  try {
    const v: unknown = JSON.parse(raw ?? "null");
    if (!v || typeof v !== "object") return { ...defaults };
    const s = v as Record<string, unknown>;
    return {
      libraryFilters: parseLibraryFilters(s.libraryFilters),
      libraryBpmRange: parseBpmRange(s.libraryBpmRange),
      jogSmoothing:
        typeof s.jogSmoothing === "number" &&
        [0, 20, 50, 60, 100].includes(s.jogSmoothing)
          ? s.jogSmoothing === 60
            ? 20
            : s.jogSmoothing
          : 0,
      playhead: s.playhead === "center" ? "center" : "third",
      playheadColor: s.playheadColor === "red" ? "red" : "white",
      window:
        typeof s.window === "number" &&
        Number.isInteger(s.window) &&
        s.window >= 2 &&
        s.window <= 32 &&
        s.window % 2 === 0
          ? s.window
          : 8,
      bass: s.bass === "balanced" ? "balanced" : "emphasis",
      time: s.time === "elapsed" ? "elapsed" : "remaining",
      overview: typeof s.overview === "boolean" ? s.overview : true,
      phrases: typeof s.phrases === "boolean" ? s.phrases : true,
      details: typeof s.details === "boolean" ? s.details : true,
    };
  } catch {
    return { ...defaults };
  }
}
export function readSettings(): Settings {
  try {
    return parseSettings(localStorage.getItem(settingsKey));
  } catch {
    return { ...defaults };
  }
}
export function timeLabel(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds)) return "—:—";
  const whole = Math.floor(Math.max(0, seconds));
  return `${String(Math.floor(whole / 60)).padStart(2, "0")}:${String(whole % 60).padStart(2, "0")}`;
}

export function playerTimeLabel(
  seconds: number | null,
  milliseconds = false,
): string {
  if (seconds === null || !Number.isFinite(seconds))
    return milliseconds ? "—:—.---" : "—M—S—";
  const value = Math.max(0, seconds);
  const whole = Math.floor(value);
  const minutes = String(Math.floor(whole / 60)).padStart(2, "0");
  const sec = String(whole % 60).padStart(2, "0");
  const fraction = milliseconds
    ? String(Math.floor((value - whole) * 1000)).padStart(3, "0")
    : (Math.floor((value - whole) * 150) / 2).toFixed(1).padStart(4, "0");
  return milliseconds
    ? `${minutes}:${sec}.${fraction}`
    : `${minutes}M${sec}S${fraction}F`;
}

export function parseBpmRange(value: unknown): { min: number; max: number } {
  if (value && typeof value === "object") {
    const v = value as Record<string, unknown>;
    if (
      typeof v.min === "number" &&
      typeof v.max === "number" &&
      Number.isInteger(v.min) &&
      Number.isInteger(v.max) &&
      v.min >= 1 &&
      v.max <= 999 &&
      v.min <= v.max
    )
      return { min: v.min, max: v.max };
  }
  return { min: 115, max: 130 };
}
