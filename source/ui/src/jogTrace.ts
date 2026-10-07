import type { Motion } from "./playbackClock";
let until = 0;
let started = 0;
let frames: object[] = [];
let startedWall: string | null = null;
let callbacks = 0,
  missingMotion = 0;
let snapshots: object[] = [];
let hostSession = "";
let cueSession = false;
const cueReports: ReturnType<typeof stopJogTrace>[] = [];
function archiveCueTrace() {
  if (cueSession && !cueReports.some((r) => r.hostSession === hostSession)) {
    cueReports.push(stopJogTrace());
    if (cueReports.length > 3) cueReports.shift();
  }
}
export function cueBrowserReports() {
  if (performance.now() > until) archiveCueTrace();
  return cueReports;
}
export function clearCueBrowserReports() {
  cueReports.length = 0;
  cueSession = false;
}

export function syncJogTrace(state?: {
  active: boolean;
  session: string;
  remainingMs?: number;
  profile?: string;
}) {
  if (!state) return;
  if (state.active && state.session && state.session !== hostSession) {
    archiveCueTrace();
    cueSession = state.profile === "cue-window";
    startJogTrace();
    hostSession = state.session;
    until =
      performance.now() +
      Math.min(45000, Math.max(0, state.remainingMs ?? 45000));
  } else if (!state.active && state.session === hostSession) {
    until = 0;
    archiveCueTrace();
  }
}
export function recordJogSnapshot(decks: unknown[]) {
  const now = performance.now();
  if (!startedWall || now > until || snapshots.length >= 2000) return;
  snapshots.push({ browserMs: now - started, decks });
}
export function startJogTrace() {
  started = performance.now();
  until = started + 45000;
  frames = [];
  snapshots = [];
  callbacks = 0;
  missingMotion = 0;
  startedWall = new Date().toISOString();
  try {
    sessionStorage.setItem("pc.jog-capture-start", startedWall);
  } catch {
    /* optional marker */
  }
}
export function recordJogFrame(
  motion: Motion | undefined,
  position: number | null,
  now: number,
  frameMs: number,
  smoothingMs: number,
  drawCostMs: number,
) {
  const callbackAt = performance.now();
  if (!startedWall || callbackAt > until || frames.length >= 12000) return;
  callbacks++;
  if (!motion) {
    missingMotion++;
    return;
  }
  frames.push({
    browserMs: callbackAt - started,
    animationFrameTime: now,
    observationId: motion.observationId,
    trackKey: motion.key,
    inputPosition: motion.position,
    motionRate: motion.rate,
    tempoRate: motion.tempoRate,
    drawnPosition: position,
    quality: motion.quality,
    direct: motion.direct,
    playing: motion.playing,
    frameMs,
    smoothingMs,
    drawCostMs,
    receiptToFrameMs: callbackAt - motion.receivedAt,
    estimatedObservationAgeMs:
      callbackAt - (motion.observedAt ?? motion.receivedAt),
  });
}
export function stopJogTrace() {
  until = 0;
  let previousStart: string | null = null;
  try {
    previousStart = sessionStorage.getItem("pc.jog-capture-start");
  } catch {
    /* optional marker */
  }
  return {
    version: 3,
    hostSession,
    startedWall,
    previousStart,
    pageReloaded: !startedWall && previousStart !== null,
    callbacks,
    missingMotion,
    snapshots,
    warnings: frames.length
      ? []
      : [
          !startedWall
            ? "No capture started in this page instance; page may have reloaded."
            : callbacks === 0
              ? "No waveform callbacks recorded; check STATUS visibility and capture duration."
              : "Waveform callbacks had no live motion.",
        ],
    frames,
    notes:
      "receiptToFrameMs is browser receipt-to-frame-callback time, not physical screen latency. Host and browser clocks are separate. No CDJ-to-client network latency is measured.",
  };
}
