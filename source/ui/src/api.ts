import type { Analysis } from "./model";
export async function loadAnalysis(file?: File): Promise<Analysis> {
  const response = await fetch(
    file ? "/api/analysis" : "/api/capture",
    file
      ? {
          method: "POST",
          body: await file.arrayBuffer(),
          headers: { "Content-Type": "application/octet-stream" },
        }
      : undefined,
  );
  if (!response.ok) throw new Error(await response.text());
  return response.json();
}
