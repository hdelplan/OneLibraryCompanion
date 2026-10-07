export type LibraryDensity = "small" | "medium" | "large";
export const densityRows: Record<LibraryDensity, number> = {
  small: 16,
  medium: 12,
  large: 8,
};
export function readLibraryDensity(): LibraryDensity {
  try {
    const value = localStorage.getItem("pc.library.density.v1");
    return value === "small" || value === "large" ? value : "medium";
  } catch {
    return "medium";
  }
}
