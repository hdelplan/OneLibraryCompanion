export const resolutions = {
  "1280x800": [1280, 800],
  "1920x720": [1920, 720],
  "1920x1080": [1920, 1080],
  "1280x720": [1280, 720],
  "1024x768": [1024, 768],
} as const;
export type Resolution = "auto" | keyof typeof resolutions;
export function parseResolution(value: unknown): Resolution {
  return typeof value === "string" && Object.hasOwn(resolutions, value)
    ? (value as keyof typeof resolutions)
    : "auto";
}

export function displayLayout(
  resolution: Resolution,
  viewportWidth: number,
  viewportHeight: number,
) {
  const availableWidth = Math.max(1, viewportWidth);
  const availableHeight = Math.max(1, viewportHeight);
  const [targetWidth, targetHeight] =
    resolution === "auto"
      ? [availableWidth, availableHeight]
      : resolutions[resolution];
  const ratio = targetWidth / targetHeight;
  // Keep controls usable on tall windows while preserving the screen ratio.
  const width = Math.max(800 * ratio, (800 * 4) / 3);
  const height = width / ratio;
  return {
    width,
    height,
    scale: Math.min(availableWidth / width, availableHeight / height),
  };
}
