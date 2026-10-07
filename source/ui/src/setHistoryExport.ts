import {
  type DJSet,
  type ExportFormat,
  orderedTracks,
  setCsv,
  setDate,
  setText,
} from "./setHistoryModel";

// Canvas text preserves Unicode without shipping a second font system. At 2x A4
// resolution the PDF prints cleanly, and the same renderer works in a WKWebView.
async function pdfBlob(sets: DJSet[]): Promise<Blob> {
  const { jsPDF } = await import("jspdf");
  const pdf = new jsPDF({ unit: "pt", format: "a4", compress: true });
  const canvas = document.createElement("canvas");
  canvas.width = 1190;
  canvas.height = 1684;
  const ctx = canvas.getContext("2d")!;
  let pages = 0,
    y = 0;
  const page = () => {
    ctx.setTransform(2, 0, 0, 2, 0, 0);
    ctx.fillStyle = "#fff";
    ctx.fillRect(0, 0, 595, 842);
    ctx.fillStyle = "#536170";
    ctx.font = "10px Arial";
    ctx.fillText("ONELIBRARYCOMPANION / SET HISTORY", 40, 35);
    y = 68;
  };
  const finish = () => {
    ctx.fillStyle = "#687888";
    ctx.font = "9px Arial";
    ctx.fillText(`OneLibraryCompanion · Page ${pages + 1}`, 40, 817);
    if (pages++) pdf.addPage();
    pdf.addImage(
      canvas.toDataURL("image/png"),
      "PNG",
      0,
      0,
      595.28,
      841.89,
      undefined,
      "FAST",
    );
  };
  const line = (
    text: string,
    size = 11,
    bold = false,
    color = "#202932",
    indent = 0,
  ) => {
    ctx.font = `${bold ? "bold " : ""}${size}px Arial`;
    const max = 515 - indent;
    const chunks: string[] = [];
    for (const paragraph of text.split(/\r?\n/)) {
      let current = "";
      for (const word of paragraph.split(/(\s+)/)) {
        if (current && ctx.measureText(current + word).width > max) {
          chunks.push(current.trimEnd());
          current = "";
        }
        const next = current ? word : word.trimStart();
        for (const char of next) {
          if (current && ctx.measureText(current + char).width > max) {
            chunks.push(current);
            current = "";
          }
          current += char;
        }
      }
      chunks.push(current.trimEnd());
    }
    for (const textLine of chunks) {
      if (y + size + 7 > 787) {
        finish();
        page();
      }
      ctx.font = `${bold ? "bold " : ""}${size}px Arial`;
      ctx.fillStyle = color;
      ctx.fillText(textLine, 40 + indent, y);
      y += size + 7;
    }
  };
  for (const [index, set] of sets.entries()) {
    if (index) finish();
    page();
    line(set.title, 20, true);
    line(setDate(set), 11, false, "#536170");
    if (set.origin === "sample")
      line("SAMPLE — not a recorded performance", 10, true, "#9b5c11");
    if (set.dateOnly) line("Start/end times unavailable", 10);
    line(`Location: ${set.location || "Not specified"}`);
    if (set.comment) line(`Comment: ${set.comment}`, 10);
    y += 12;
    line(`${set.order.length} TRACKS`, 10, true, "#536170");
    for (const [i, { track }] of orderedTracks(set).entries()) {
      if (y > 737) {
        finish();
        page();
        line(`${set.title} / continued`, 12, true);
      }
      line(`${String(i + 1).padStart(2, "0")}   ${track.title}`, 12, true);
      line(
        `${track.artist || "Unknown artist"}  ·  ${track.key || "No key"}  ·  ${track.rating}/5 stars`,
        10,
        false,
        "#536170",
        24,
      );
      y += 6;
    }
  }
  finish();
  return pdf.output("blob");
}
export async function exportFile(
  sets: DJSet[],
  format: ExportFormat,
): Promise<File> {
  const name =
    sets.length === 1
      ? `${sets[0].title}-${new Date(sets[0].startedAt).toISOString().slice(0, 10)}`
      : "OneLibraryCompanion-set-history";
  const safe = name.replace(/[^\p{L}\p{N}._-]+/gu, "-").slice(0, 100);
  const blob =
    format === "pdf"
      ? await pdfBlob(sets)
      : new Blob([format === "csv" ? setCsv(sets) : setText(sets)], {
          type:
            format === "csv"
              ? "text/csv;charset=utf-8"
              : "text/plain;charset=utf-8",
        });
  return new File([blob], `${safe}.${format}`, { type: blob.type });
}
export function download(file: File) {
  const url = URL.createObjectURL(file);
  const a = document.createElement("a");
  a.href = url;
  a.download = file.name;
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}
export function emailDraft(file: File, sets: DJSet[]) {
  download(file);
  const body =
    file.name.endsWith(".txt") && setText(sets).length < 5000
      ? setText(sets)
      : `Please attach the downloaded file: ${file.name}\n\n${sets.map((s) => `${s.title} — ${setDate(s)}`).join("\n")}`;
  const a = document.createElement("a");
  a.href = `mailto:?subject=${encodeURIComponent("DJ set history")}&body=${encodeURIComponent(body)}`;
  a.click();
}
