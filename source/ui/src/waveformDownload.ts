import type { Analysis, Wave } from "./model";

// Large waveform JSON should survive slow Wi-Fi as long as data keeps arriving.
// The caller cancels a superseded track; stalled transfers stop after 15 seconds.
export async function downloadWaveform(
  number: number,
  key: string,
  controller: AbortController,
  request: typeof fetch = fetch,
): Promise<Analysis | undefined> {
  let idle = setTimeout(() => controller.abort(), 15000);
  const deadline = setTimeout(() => controller.abort(), 120000);
  const progress = () => {
    clearTimeout(idle);
    idle = setTimeout(() => controller.abort(), 15000);
  };
  try {
    const response = await request(
      `/api/live/analysis/${number}?waveform=packed`,
      {
        signal: controller.signal,
        // The same endpoint serves a different response after every track load.
        cache: "no-store",
      },
    );
    if (!response.ok) return;
    progress();
    let result: { key: string; analysis: Analysis };
    if (response.body) {
      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      const parts: string[] = [];
      try {
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          if (controller.signal.aborted) return;
          progress();
          parts.push(decoder.decode(value, { stream: true }));
        }
        parts.push(decoder.decode());
        result = JSON.parse(parts.join(""));
      } finally {
        if (controller.signal.aborted) await reader.cancel().catch(() => {});
        reader.releaseLock();
      }
    } else {
      result = await response.json();
    }
    if (!controller.signal.aborted && result.key === key)
      return unpackWaveform(result.analysis);
  } finally {
    clearTimeout(idle);
    clearTimeout(deadline);
  }
}

export function unpackWaveform(analysis: Analysis): Analysis {
  for (const field of ["detail", "preview"] as const) {
    const wave = analysis[field] as (Wave & { packedColumns?: string }) | null;
    if (!wave || wave.packedColumns === undefined) continue;
    const packed = wave.packedColumns;
    if (
      !/^(?:[0-9a-f]{6})*$/i.test(packed) ||
      !["PWV6", "PWV7"].includes(wave.tag) ||
      ![127, 255].includes(wave.normalization)
    )
      throw new Error("Invalid packed waveform");
    const detail = wave.tag === "PWV7";
    const rawColumns: number[][] = [];
    const columns: Wave["columns"] = [];
    for (let i = 0; i < packed.length; i += 6) {
      const raw = [0, 2, 4].map((offset) =>
        parseInt(packed.slice(i + offset, i + offset + 2), 16),
      );
      if (detail && raw.some((v) => v > 127))
        throw new Error("Invalid waveform height");
      rawColumns.push(raw);
      columns.push({
        low: Math.fround(raw[detail ? 0 : 2] / wave.normalization),
        mid: Math.fround(raw[detail ? 1 : 0] / wave.normalization),
        high: Math.fround(raw[detail ? 2 : 1] / wave.normalization),
      });
    }
    analysis[field] = {
      tag: wave.tag,
      normalization: wave.normalization,
      samplesPerSecond: wave.samplesPerSecond,
      rawColumns,
      columns,
    };
  }
  return analysis;
}
