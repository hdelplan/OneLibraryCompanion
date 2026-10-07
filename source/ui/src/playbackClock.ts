export type Motion = {
  direct?: boolean;
  cued?: boolean;
  observedAt?: number;
  observationId?: string;
  quality?: "beat" | "fine" | "held" | "coarse" | "unavailable";
  smoothMs?: number;
  loop?: { start: number; end: number };
  beatNumber?: number;
  // Position is a whole-beat anchor, with no measured sub-beat phase.
  beatOnly?: boolean;
  key: string;
  position: number;
  receivedAt: number;
  playing: boolean;
  rate: number;
  // Effective tempo for waveform scale; manual nudges change motion, not zoom.
  tempoRate?: number;
};
// Steady transport advances at pitch rate, with bounded coarse-phase correction.
// Manual movement uses bounded transitions; seeks and loop wraps are discontinuities.
export class PlaybackClock {
  private sample: Motion | null = null;
  private position = 0;
  private frameAt = 0;
  private direction = 0;
  private coarseCalibrated = false;
  private phaseOffset = 0;
  private phaseTarget = 0;
  private phaseEvidence: number[] = [];
  private glide: { from: number; start: number; duration: number } | null =
    null;
  update(next: Motion) {
    if (
      this.sample &&
      next.receivedAt === this.sample.receivedAt &&
      next.observationId === this.sample.observationId &&
      next.key === this.sample.key
    )
      return;
    if (
      this.sample &&
      next.key === this.sample.key &&
      next.observationId &&
      next.observationId === this.sample.observationId &&
      next.position === this.sample.position &&
      next.direct === this.sample.direct &&
      next.playing === this.sample.playing &&
      next.rate === this.sample.rate &&
      next.quality === this.sample.quality &&
      next.beatOnly === this.sample.beatOnly &&
      next.beatNumber === this.sample.beatNumber &&
      next.loop?.start === this.sample.loop?.start &&
      next.loop?.end === this.sample.loop?.end
    ) {
      this.sample = {
        ...this.sample,
        observedAt: Math.min(
          this.sample.observedAt ?? this.sample.receivedAt,
          next.observedAt ?? next.receivedAt,
        ),
      };
      return;
    }
    const effectiveAt = Math.max(next.receivedAt, this.frameAt);
    const current = this.read(effectiveAt);
    if (
      (next.quality === "beat" ||
        next.quality === "fine" ||
        next.quality === "held") &&
      next.playing &&
      !next.direct
    ) {
      // The host integrates motion speed from a real beat arrival. Correct to
      // that observation immediately, including known host/browser age, without
      // applying the status-boundary median/slew used by the coarse fallback.
      this.position =
        next.position +
        (Math.max(
          0,
          Math.min(1000, effectiveAt - (next.observedAt ?? next.receivedAt)),
        ) /
          1000) *
          next.rate;
      this.sample = next;
      this.frameAt = effectiveAt;
      this.coarseCalibrated = false;
      this.phaseOffset = this.phaseTarget = 0;
      this.phaseEvidence = [];
      this.direction = 0;
      this.glide = null;
      return;
    }
    const sameTrack = this.sample?.key === next.key;
    const sameBeat =
      next.beatNumber !== undefined &&
      next.beatNumber === this.sample?.beatNumber;
    if (
      sameTrack &&
      sameBeat &&
      next.direct &&
      !next.cued &&
      next.quality === "coarse"
    ) {
      // A coarse beat boundary cannot improve the last on-screen sub-beat
      // position during a jog/hold. Wait for fine data or a changed beat.
      // Stop at the observation time, not at its later browser delivery. Once
      // held, repeated coarse packets cannot rewind or resume the waveform.
      const lag =
        this.sample?.playing && !this.sample.direct
          ? Math.max(
              0,
              Math.min(
                effectiveAt,
                (this.sample.observedAt ?? this.sample.receivedAt) + 1000,
              ) -
                Math.max(
                  next.observedAt ?? next.receivedAt,
                  this.sample.observedAt ?? this.sample.receivedAt,
                ),
            ) / 1000
          : 0;
      let held = Math.max(0, current - lag * (this.sample?.rate ?? 0));
      if (next.loop && next.loop.end > next.loop.start)
        held =
          next.loop.start +
          ((((held - next.loop.start) % (next.loop.end - next.loop.start)) +
            next.loop.end -
            next.loop.start) %
            (next.loop.end - next.loop.start));
      next = { ...next, position: held };
    }
    const delta = this.sample ? next.position - this.sample.position : 0;
    const direction = Math.sign(delta);
    const reversal =
      next.direct &&
      direction !== 0 &&
      this.direction !== 0 &&
      direction !== this.direction;
    if (!next.direct || next.key !== this.sample?.key) this.direction = 0;
    if (next.direct && direction !== 0) this.direction = direction;
    const glide =
      !reversal &&
      next.direct &&
      (next.smoothMs ?? 0) > 0 &&
      this.sample?.direct &&
      next.key === this.sample.key &&
      next.playing === this.sample.playing &&
      Math.abs(next.position - current) < 0.75 &&
      !(next.loop && this.sample && next.position < this.sample.position);
    // Never predict beyond a received position. Reversals immediately retarget
    // from the current screen position; large seeks and loop wraps snap.
    if (glide) {
      if (next.position !== this.sample!.position)
        this.glide = {
          from: current,
          start: next.receivedAt,
          duration: next.smoothMs!,
        };
      this.sample = next;
      this.frameAt = effectiveAt;
      return;
    }
    this.glide = null;
    const discontinuity =
      !sameTrack ||
      next.direct ||
      !next.playing ||
      !next.beatOnly ||
      this.sample?.rate !== next.rate ||
      (next.beatNumber !== undefined &&
        this.sample?.beatNumber !== undefined &&
        next.beatNumber < this.sample.beatNumber &&
        !next.loop) ||
      Math.abs(next.position - current) >
        (next.loop ? next.loop.end - next.loop.start : 0.75);
    if (discontinuity) {
      this.coarseCalibrated = false;
      this.phaseOffset = this.phaseTarget = 0;
      this.phaseEvidence = [];
    }
    const boundaryPosition = statusBeatPosition(this.sample, next, effectiveAt);
    let initialPhase: number | null = null;
    if (boundaryPosition !== null) {
      const estimate = this.phaseOffset + boundaryPosition - current;
      this.phaseEvidence.push(estimate);
      if (this.phaseEvidence.length > 9) this.phaseEvidence.shift();
      const sorted = [...this.phaseEvidence].sort((a, b) => a - b);
      if (!this.coarseCalibrated) {
        initialPhase = boundaryPosition;
        this.phaseOffset = estimate;
        this.phaseTarget = estimate;
        this.coarseCalibrated = true;
      } else if (sorted.length >= 3) {
        this.phaseTarget = sorted[Math.floor(sorted.length / 2)];
      }
    }
    if (
      !this.sample ||
      next.direct ||
      (next.beatNumber !== undefined &&
        this.sample.beatNumber !== undefined &&
        !next.loop &&
        next.beatNumber < this.sample.beatNumber) ||
      next.key !== this.sample.key ||
      next.playing !== this.sample.playing ||
      Math.abs(next.position - current) >
        (next.loop ? next.loop.end - next.loop.start : 0.75)
    )
      this.position = next.position;
    if (
      this.sample?.direct &&
      !next.direct &&
      !(sameTrack && next.quality === "coarse" && sameBeat)
    )
      this.position = next.position;
    if (initialPhase !== null) this.position = initialPhase;
    this.sample = next;
    this.frameAt = effectiveAt;
  }
  read(now: number) {
    const s = this.sample;
    if (!s) return this.position;
    if (s.direct && this.glide) {
      const t = Math.max(
        0,
        Math.min(1, (now - this.glide.start) / this.glide.duration),
      );
      this.position = this.glide.from + (s.position - this.glide.from) * t;
      if (t === 1) this.glide = null;
      this.frameAt = now;
      return this.position;
    }
    const until = Math.min(now, (s.observedAt ?? s.receivedAt) + 1000);
    const dt = Math.max(0, (until - this.frameAt) / 1000);
    if (s.playing && !s.direct && dt > 0) {
      // Preserve forward transport; phase correction is independent of packet
      // frequency and limited per elapsed second.
      // Only normal coarse transport gets bounded phase discipline. Manual
      // movement and measured positions never pass through this adjustment.
      const remaining =
        s.beatOnly && !s.loop ? this.phaseTarget - this.phaseOffset : 0;
      const correction = Math.max(-0.05 * dt, Math.min(0.05 * dt, remaining));
      this.phaseOffset += correction;
      this.position = Math.max(0, this.position + dt * s.rate + correction);
    } else if (!s.playing || s.direct) this.position = s.position;
    this.frameAt = Math.max(this.frameAt, until);
    if (
      s.playing &&
      !s.direct &&
      s.loop &&
      s.loop.end > s.loop.start &&
      this.position >= s.loop.end
    ) {
      this.position =
        s.loop.start +
        ((this.position - s.loop.start) % (s.loop.end - s.loop.start));
    }
    return this.position;
  }
}

function statusBeatPosition(
  previous: Motion | null,
  next: Motion,
  now: number,
): number | null {
  if (
    !previous ||
    previous.key !== next.key ||
    !previous.beatOnly ||
    !next.beatOnly ||
    !previous.playing ||
    !next.playing ||
    previous.direct ||
    next.direct ||
    next.rate <= 0 ||
    previous.rate !== next.rate ||
    previous.beatNumber === undefined ||
    next.beatNumber !== previous.beatNumber + 1 ||
    previous.beatNumber === 0 ||
    next.position <= previous.position
  )
    return null;
  const before = previous.observedAt ?? previous.receivedAt;
  const after = next.observedAt ?? next.receivedAt;
  const gap = after - before;
  // A real consecutive beat transition brackets the boundary between these
  // two packets. Repeated same-beat packets do not tell us the phase. Reject
  // wide/stale gaps rather than pretending they are accurate beat timestamps.
  const beatMs = ((next.position - previous.position) / next.rate) * 1000;
  if (gap <= 0 || gap > Math.min(250, beatMs) || now - after > 1000)
    return null;
  const earliest =
    next.position + (Math.max(0, now - after) / 1000) * next.rate;
  const latest = next.position + (Math.max(0, now - before) / 1000) * next.rate;
  // This is uncertain evidence, not an exact beat timestamp. The caller
  // combines several transitions before adjusting an established clock.
  return (earliest + latest) / 2;
}
