export interface CaptureState {
  active: boolean;
  session: string;
  remainingMs: number;
  profile?: string | null;
  syncTap?: {
    sync: boolean | null;
    edges: number;
    pulses: number;
    shortPulseCandidates: number;
  } | null;
  message?: string | null;
  players?: Array<{
    number: number;
    master: boolean | null;
    masterMeaningful: boolean | null;
    yieldingTo: number | null;
  }> | null;
}
export function handoffPrompt(state: CaptureState): string {
  if (!state.active)
    return (
      state.message ||
      "Capture ended. Restore CDJ1 MASTER manually, then stop and prepare the report."
    );
  const masters =
    state.players?.filter(
      (p) =>
        p.master === true &&
        p.masterMeaningful === true &&
        p.yieldingTo === null,
    ) ?? [];
  const reported = masters.length === 1 ? masters[0].number : null;
  const anyOtherMaster = state.players?.some(
    (p) => p.number !== reported && (p.master || p.masterMeaningful),
  );
  const master = anyOtherMaster ? null : reported;
  const remaining = Math.ceil(state.remainingMs / 1000);
  if (state.remainingMs > 33000) {
    return `${master === 1 ? "Keep CDJ1 master. Jog CDJ2 forward and backward." : "Restore CDJ1 MASTER and wait for confirmation."} ${remaining} seconds remaining.`;
  }
  if (state.remainingMs > 20000) {
    return `${master === 2 ? "CDJ2 master confirmed. Continue small forward/backward movements on CDJ2." : "Press CDJ2 MASTER. Wait for confirmation, then continue jogging CDJ2."} ${remaining} seconds remaining.`;
  }
  return `${master === 1 ? "CDJ1 master restored. Continue small forward/backward movements on CDJ2 to test whether fine updates persist." : "Press CDJ1 MASTER now. Wait for confirmation, then continue jogging CDJ2."} ${remaining} seconds remaining.`;
}

export function syncTapPrompt(state: CaptureState): string {
  if (!state.active)
    return (
      state.message ||
      "Capture ended. Ensure CDJ2 Sync is OFF, then stop and prepare the report."
    );
  const ms = state.remainingMs;
  const instruction =
    ms > 40000
      ? "Wait with CDJ2 Sync OFF. Do not touch MASTER, Play or the jog wheel."
      : ms > 35000
        ? "Slow control: press CDJ2 SYNC twice, about 1 second apart. One pair only, then wait."
        : ms > 30000
          ? "Wait. If CDJ2 Sync remains ON, switch it OFF before the quick attempts."
          : ms > 25000
            ? "Quick attempt 1: double-tap CDJ2 SYNC once, then wait."
            : ms > 20000
              ? "Wait. If CDJ2 Sync remains ON, switch it OFF."
              : ms > 15000
                ? "Quick attempt 2: double-tap CDJ2 SYNC once, then wait."
                : ms > 10000
                  ? "Wait. If CDJ2 Sync remains ON, switch it OFF."
                  : ms > 5000
                    ? "Quick attempt 3: double-tap CDJ2 SYNC once, then wait."
                    : "Ensure CDJ2 Sync is OFF, then hold still until the capture ends.";
  return `${instruction} ${Math.ceil(ms / 1000)} seconds remaining.`;
}
