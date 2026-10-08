import type { LibraryTrack } from "./libraryModel";
import type { LivePlayer } from "./model";

// USB/Link audio limits, not library-database compatibility. Unknown metadata
// is deliberately inconclusive. See docs/research/local-usb-onelibrary.md.
export function unsupportedReason(
  track: LibraryTrack,
  player: LivePlayer | undefined,
): string | null {
  if (player?.connection !== "connected") return null;
  const model = player.name.toUpperCase().replace(/[^A-Z0-9]/g, "");
  const nexus = ["CDJ2000", "CDJ2000NEXUS", "CDJ2000NXS"].includes(model);
  const nxs2 = ["CDJ2000NXS2", "CDJ2000NEXUS2"].includes(model);
  const cdj3000 = model === "CDJ3000";
  if (!nexus && !nxs2 && !cdj3000) return null;
  const format =
    typeof track.format === "string" ? track.format.toLowerCase() : "";
  const lossless = ["wav", "aiff", "flac", "alac"].includes(format);
  if (!["mp3", "aac"].includes(format) && !lossless) return null;
  if (nexus && ["flac", "alac"].includes(format))
    return `${player.name} cannot play ${format.toUpperCase()}`;
  const rates = lossless
    ? nexus
      ? [44100, 48000]
      : [44100, 48000, 88200, 96000]
    : cdj3000
      ? [44100, 48000]
      : format === "mp3"
        ? [32000, 44100, 48000]
        : [16000, 22050, 24000, 32000, 44100, 48000];
  if (
    typeof track.sampleRate === "number" &&
    track.sampleRate > 0 &&
    !rates.includes(track.sampleRate)
  )
    return `${player.name} cannot play ${format.toUpperCase()} at ${track.sampleRate / 1000} kHz`;
  if (
    lossless &&
    typeof track.sampleDepth === "number" &&
    track.sampleDepth > 0 &&
    ![16, 24].includes(track.sampleDepth)
  )
    return `${player.name} cannot play ${track.sampleDepth}-bit ${format.toUpperCase()}`;
  return null;
}

export function incompatiblePlayers(
  track: LibraryTrack,
  players: LivePlayer[],
) {
  return players.filter((player) => unsupportedReason(track, player) !== null);
}

// Conversion is available only for host-attached media and known destinations.
export function canTranscode(
  track: LibraryTrack,
  sourceId: string | undefined,
  player?: LivePlayer,
): boolean {
  const model = player?.name.toUpperCase().replace(/[^A-Z0-9]/g, "") ?? "";
  return (
    !!sourceId?.startsWith("local-usb:") &&
    player?.connection === "connected" &&
    [
      "CDJ2000",
      "CDJ2000NEXUS",
      "CDJ2000NXS",
      "CDJ2000NXS2",
      "CDJ2000NEXUS2",
      "CDJ3000",
    ].includes(model) &&
    ["wav", "aiff", "flac", "alac", "mp3", "aac"].includes(
      String(track.format).toLowerCase(),
    )
  );
}
