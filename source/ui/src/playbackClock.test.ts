import { test } from "node:test";
import assert from "node:assert/strict";
import { PlaybackClock } from "./playbackClock";
const sample = {
  key: "a",
  position: 10,
  receivedAt: 0,
  playing: true,
  rate: 1,
};
test("whole-beat startup offsets converge at a real beat transition for either deck", () => {
  for (const key of ["deck1", "deck2"]) {
    for (const offset of [0.25, 0.45]) {
      const clock = new PlaybackClock();
      const rate = 1.0118;
      let lastPacket = -64;
      for (let now = 0; now <= 4000; now += 16) {
        const actual = 10 + offset + (now / 1000) * rate;
        if (now - lastPacket >= 64) {
          const beat = Math.floor(actual / 0.5);
          clock.update({
            ...sample,
            key,
            rate,
            position: beat * 0.5,
            beatNumber: beat + 1,
            beatOnly: true,
            quality: "coarse",
            receivedAt: now + 30,
            observedAt: now,
            observationId: String(now),
          });
          lastPacket = now;
        }
        const shown = clock.read(now + 30);
        if (now > 600)
          assert.ok(
            Math.abs(shown - (actual + 0.03 * rate)) < 0.04,
            `${key}: persistent error ${shown - actual} at ${now}`,
          );
      }
    }
  }
});
test("beat evidence corrects a large offset once then preserves steady frame speed", () => {
  const clock = new PlaybackClock();
  const coarse = {
    ...sample,
    beatOnly: true,
    quality: "coarse" as const,
    beatNumber: 21,
  };
  clock.update(coarse);
  clock.update({ ...coarse, receivedAt: 64 });
  clock.update({ ...coarse, position: 10.5, beatNumber: 22, receivedAt: 128 });
  assert.equal(clock.read(128), 10.532);
  assert.ok(Math.abs(clock.read(144) - 10.548) < 1e-9);
  // The next boundary interval contains the running clock: no correction.
  clock.update({ ...coarse, position: 10.5, beatNumber: 22, receivedAt: 576 });
  const before = clock.read(640);
  clock.update({ ...coarse, position: 11, beatNumber: 23, receivedAt: 640 });
  assert.ok(Math.abs(clock.read(640) - before) < 1e-9);
});
test("master handoff to beat-only data recovers phase without locking decks together", () => {
  const clock = new PlaybackClock();
  clock.update({ ...sample, position: 10.35, beatNumber: 21, quality: "fine" });
  clock.update({
    ...sample,
    receivedAt: 64,
    observedAt: 64,
    beatNumber: 21,
    beatOnly: true,
    quality: "coarse",
  });
  clock.update({
    ...sample,
    receivedAt: 128,
    observedAt: 128,
    position: 10.5,
    beatNumber: 22,
    beatOnly: true,
    quality: "coarse",
  });
  assert.ok(Math.abs(clock.read(128) - 10.532) < 1e-9);
});
test("beat-only four-beat loops retain calibrated phase across repeated wraps", () => {
  const clock = new PlaybackClock();
  const loop = { start: 10, end: 12 };
  for (let now = 0; now < 8000; now += 16) {
    const actual = 10 + ((0.35 + now / 1000) % 2);
    if (now % 64 === 0) {
      const index = Math.floor((actual - 10) / 0.5);
      clock.update({
        ...sample,
        loop,
        position: 10 + index * 0.5,
        beatNumber: 21 + index,
        beatOnly: true,
        quality: "coarse",
        receivedAt: now,
      });
    }
    const displayed = clock.read(now);
    const distance = Math.abs(displayed - actual);
    if (now > 600) assert.ok(Math.min(distance, 2 - distance) < 0.04);
  }
});
test("same-beat packets, stale gaps, manual movement and measured phase cannot calibrate from coarse boundaries", () => {
  for (const overrides of [
    { beatNumber: 21, position: 10 },
    { receivedAt: 600 },
    { direct: true },
    { beatOnly: false },
    { key: "new-track" },
  ]) {
    const clock = new PlaybackClock();
    clock.update({ ...sample, beatNumber: 21, beatOnly: true });
    clock.update({
      ...sample,
      position: 10.5,
      beatNumber: 22,
      beatOnly: true,
      receivedAt: 64,
      ...overrides,
    });
    const value = clock.read(overrides.receivedAt ?? 64);
    assert.notEqual(value, 10.532);
  }
});
test("60 Hz motion remains continuous between 5 Hz status updates", () => {
  const clock = new PlaybackClock();
  clock.update(sample);
  let last = 10;
  for (let frame = 1; frame <= 60; frame++) {
    const now = (frame * 1000) / 60;
    if (frame % 12 === 0)
      clock.update({
        ...sample,
        receivedAt: now,
        position: 10 + now / 1000 + 0.04,
      });
    const position = clock.read(now);
    assert.ok(position > last);
    assert.ok(position - last < 0.021);
    last = position;
  }
  assert.ok(Math.abs(last - 11) < 0.06);
});
test("pause, seek and track changes re-anchor; stale updates stop advancing", () => {
  const clock = new PlaybackClock();
  clock.update(sample);
  assert.equal(clock.read(2000), 11);
  assert.equal(clock.read(3000), 11);
  clock.update({ ...sample, receivedAt: 3100, position: 30 });
  assert.equal(clock.read(3100), 30);
  clock.update({ ...sample, receivedAt: 3200, position: 30.1, playing: false });
  assert.equal(clock.read(4000), 30.1);
  clock.update({ ...sample, key: "b", receivedAt: 4100, position: 0 });
  assert.equal(clock.read(4100), 0);
});
test("pitch controls interpolation rate", () => {
  const clock = new PlaybackClock();
  clock.update({ ...sample, rate: 1.08 });
  assert.ok(Math.abs(clock.read(500) - 10.54) < 1e-9);
});

test("jog smoothing settles within its window, reverses immediately and never overshoots", () => {
  const clock = new PlaybackClock();
  clock.update({ ...sample, direct: true, smoothMs: 60 });
  clock.update({
    ...sample,
    direct: true,
    smoothMs: 60,
    position: 10.3,
    receivedAt: 100,
  });
  assert.equal(clock.read(100), 10);
  assert.ok(Math.abs(clock.read(130) - 10.15) < 1e-9);
  clock.update({
    ...sample,
    direct: true,
    smoothMs: 60,
    position: 9.9,
    receivedAt: 130,
  });
  assert.equal(clock.read(130), 9.9);
  assert.equal(clock.read(140), 9.9);
  assert.equal(clock.read(190), 9.9);
  assert.equal(clock.read(1000), 9.9);
  clock.update({
    ...sample,
    direct: true,
    smoothMs: 60,
    position: 3,
    receivedAt: 1100,
  });
  assert.equal(clock.read(1100), 3);
});

test("repeated status with unchanged position does not restart a jog transition", () => {
  const clock = new PlaybackClock();
  clock.update({ ...sample, direct: true, smoothMs: 60 });
  clock.update({
    ...sample,
    direct: true,
    smoothMs: 60,
    position: 10.3,
    receivedAt: 100,
  });
  clock.update({
    ...sample,
    direct: true,
    smoothMs: 60,
    position: 10.3,
    receivedAt: 120,
  });
  assert.equal(clock.read(160), 10.3);
});

test("small backwards jogs and manual transport bypass easing", () => {
  const clock = new PlaybackClock();
  clock.update({ ...sample, beatNumber: 21 });
  clock.read(100);
  clock.update({ ...sample, position: 9.95, receivedAt: 110, beatNumber: 20 });
  assert.equal(clock.read(110), 9.95);
  clock.update({ ...sample, position: 10.05, receivedAt: 140, direct: true });
  assert.equal(clock.read(150), 10.05);
  clock.update({ ...sample, position: 10.01, receivedAt: 170, direct: true });
  assert.equal(clock.read(180), 10.01);
});

test("loop wraps jump to the start and pause does not keep advancing", () => {
  const clock = new PlaybackClock();
  const looping = { ...sample, position: 10.9, loop: { start: 10, end: 11 } };
  clock.update(looping);
  assert.ok(Math.abs(clock.read(200) - 10.1) < 1e-9);
  clock.update({
    ...looping,
    position: 10.15,
    receivedAt: 250,
    playing: false,
  });
  assert.equal(clock.read(500), 10.15);
  clock.update({ ...looping, position: 10.15, receivedAt: 600, playing: true });
  assert.ok(clock.read(700) > 10.15);
});

test("normal playback keeps exact pitch speed despite noisy status positions", () => {
  const clock = new PlaybackClock();
  clock.update(sample);
  let previous = 10;
  for (let i = 1; i <= 240; i++) {
    const now = (i * 1000) / 60;
    if (i % 8 === 0)
      clock.update({
        ...sample,
        receivedAt: now,
        position: 10 + now / 1000 + (i % 16 ? 0.2 : -0.2),
      });
    const position = clock.read(now);
    assert.ok(Math.abs(position - previous - 1 / 60) < 1e-8);
    previous = position;
  }
});

test("loop wrap reports cannot rewind the independently wrapped display twice", () => {
  const clock = new PlaybackClock();
  const loop = { start: 10, end: 12 };
  clock.update({ ...sample, position: 11.9, beatNumber: 24, loop });
  assert.ok(Math.abs(clock.read(200) - 10.1) < 1e-8);
  clock.update({
    ...sample,
    position: 10.02,
    receivedAt: 200,
    beatNumber: 21,
    loop,
  });
  assert.ok(Math.abs(clock.read(300) - 10.2) < 1e-8);
});

test("late React delivery cannot count previously rendered time twice", () => {
  const clock = new PlaybackClock();
  clock.update(sample);
  assert.equal(clock.read(120), 10.12);
  clock.update({ ...sample, receivedAt: 110, position: 10.11 });
  assert.ok(Math.abs(clock.read(130) - 10.13) < 1e-9);
});

test("responsive jog reaches received position in 20ms and reverses without overshoot", () => {
  const clock = new PlaybackClock();
  const jog = { ...sample, playing: false, direct: true, smoothMs: 20 };
  clock.update(jog);
  clock.update({ ...jog, position: 10.2, receivedAt: 64 });
  assert.ok(Math.abs(clock.read(74) - 10.1) < 1e-9);
  assert.equal(clock.read(84), 10.2);
  clock.update({ ...jog, position: 10.1, receivedAt: 128 });
  assert.ok(clock.read(138) < 10.2);
  assert.equal(clock.read(148), 10.1);
  assert.equal(clock.read(200), 10.1);
});

test("delayed frames consume smoothing time instead of adding another delay", () => {
  const clock = new PlaybackClock();
  clock.update({ ...sample, direct: true, smoothMs: 20 });
  clock.read(120);
  clock.update({
    ...sample,
    direct: true,
    smoothMs: 20,
    receivedAt: 110,
    position: 10.2,
  });
  assert.equal(clock.read(130), 10.2);
});

test("coarse fallback holds a jog position until a beat changes or fine data returns", () => {
  const clock = new PlaybackClock();
  const jog = {
    ...sample,
    direct: true,
    beatNumber: 21,
    quality: "fine" as const,
    position: 10.3,
  };
  clock.update(jog);
  clock.update({ ...jog, quality: "held", receivedAt: 200 });
  clock.update({ ...jog, quality: "coarse", position: 10, receivedAt: 400 });
  assert.equal(clock.read(450), 10.3);
  clock.update({
    ...jog,
    quality: "coarse",
    position: 9.5,
    beatNumber: 20,
    receivedAt: 500,
  });
  assert.equal(clock.read(550), 9.5);
  clock.update({ ...jog, position: 9.7, beatNumber: 20, receivedAt: 600 });
  assert.equal(clock.read(650), 9.7);
});
test("release from a jog into coarse playback does not snap to the beat start", () => {
  const clock = new PlaybackClock();
  clock.update({
    ...sample,
    direct: true,
    quality: "fine",
    position: 10.3,
    beatNumber: 21,
  });
  clock.update({
    ...sample,
    quality: "coarse",
    position: 10,
    beatNumber: 21,
    receivedAt: 200,
  });
  assert.equal(clock.read(200), 10.3);
  assert.ok(Math.abs(clock.read(300) - 10.4) < 1e-9);
});
test("replayed observations cannot renew extrapolation or re-anchor the same packet", () => {
  const clock = new PlaybackClock();
  const observed = { ...sample, observationId: "session:1", observedAt: -200 };
  clock.update(observed);
  for (let now = 20; now <= 2000; now += 20) {
    if (now % 100 === 0) clock.update({ ...observed, receivedAt: now });
    clock.read(now);
  }
  assert.ok(Math.abs(clock.read(2100) - 10.8) < 1e-8);
  clock.update({
    ...observed,
    observationId: "session:2",
    observedAt: 2200,
    receivedAt: 2200,
    position: 10.8,
  });
  assert.ok(clock.read(2300) > 10.8);
});
test("alternating coarse boundary jitter cannot repeatedly retime steady playback", () => {
  const clock = new PlaybackClock();
  const base = { ...sample, beatOnly: true, quality: "coarse" as const };
  clock.update({ ...base, beatNumber: 21 });
  clock.update({ ...base, beatNumber: 21, receivedAt: 64 });
  clock.update({ ...base, position: 10.5, beatNumber: 22, receivedAt: 128 });
  for (let i = 0; i < 8; i++) {
    const boundary = 596 + 500 * i + (i % 2 ? 120 : -120);
    clock.update({
      ...base,
      position: 10.5 + 0.5 * i,
      beatNumber: 22 + i,
      receivedAt: boundary - 30,
    });
    const before = clock.read(boundary + 30);
    clock.update({
      ...base,
      position: 11 + 0.5 * i,
      beatNumber: 23 + i,
      receivedAt: boundary + 30,
    });
    assert.ok(Math.abs(clock.read(boundary + 30) - before) < 1e-8);
  }
});
test("coarse phase discipline converges after an unreported offset without instantaneous jumps", () => {
  const clock = new PlaybackClock();
  let lastPacket = -1000;
  let last = 0;
  const gaps = [64, 112, 176, 80, 144];
  let packet = 0;
  let worstLateError = 0;
  for (let now = 0; now <= 16000; now += 16) {
    const actual = 10.35 + now / 1000 + (now >= 6000 ? 0.12 : 0);
    if (now - lastPacket >= gaps[packet % gaps.length]) {
      const beat = Math.floor(actual / 0.5);
      const before = clock.read(now);
      clock.update({
        ...sample,
        position: beat * 0.5,
        beatNumber: beat + 1,
        beatOnly: true,
        quality: "coarse",
        receivedAt: now,
        observedAt: now,
        observationId: String(packet++),
      });
      if (now > 1000)
        assert.ok(
          Math.abs(clock.read(now) - before) < 1e-8,
          `instantaneous correction at ${now}`,
        );
      lastPacket = now;
    }
    const shown = clock.read(now);
    if (now > 1000)
      assert.ok(
        Math.abs(shown - last - 0.016) <= 0.000801,
        `rate outside correction bound at ${now}`,
      );
    if (now > 11000)
      worstLateError = Math.max(worstLateError, Math.abs(shown - actual));
    last = shown;
  }
  assert.ok(worstLateError < 0.07, `unresolved phase error ${worstLateError}`);
});
test("manual reversals bypass coarse phase correction immediately", () => {
  const clock = new PlaybackClock();
  clock.update({
    ...sample,
    beatOnly: true,
    quality: "coarse",
    beatNumber: 21,
  });
  clock.update({
    ...sample,
    position: 10.5,
    beatOnly: true,
    quality: "coarse",
    beatNumber: 22,
    receivedAt: 128,
  });
  clock.update({
    ...sample,
    position: 10.9,
    direct: true,
    quality: "fine",
    receivedAt: 144,
  });
  assert.equal(clock.read(144), 10.9);
  clock.update({
    ...sample,
    position: 10.6,
    direct: true,
    quality: "fine",
    receivedAt: 160,
  });
  assert.equal(clock.read(160), 10.6);
});

test("beat anchors apply immediately and motion speed responds without coarse phase slew", () => {
  const clock = new PlaybackClock();
  clock.update({ ...sample, quality: "beat", observedAt: 0, receivedAt: 30 });
  assert.ok(Math.abs(clock.read(30) - 10.03) < 1e-9);
  clock.update({
    ...sample,
    quality: "beat",
    position: 10.1,
    rate: 0.8,
    observedAt: 100,
    receivedAt: 130,
  });
  assert.ok(Math.abs(clock.read(130) - 10.124) < 1e-9);
  assert.ok(Math.abs(clock.read(230) - 10.204) < 1e-9);
  // A real beat replaces the prior estimate; no gradual catch-up is introduced.
  clock.update({
    ...sample,
    quality: "beat",
    position: 10.5,
    rate: 0.8,
    observedAt: 500,
    receivedAt: 530,
  });
  assert.ok(Math.abs(clock.read(530) - 10.524) < 1e-9);
});
test("repeated beat observations cannot extend the motion timeout", () => {
  const clock = new PlaybackClock();
  clock.update({
    ...sample,
    quality: "beat",
    observationId: "one",
    observedAt: 0,
  });
  clock.read(500);
  clock.update({
    ...sample,
    quality: "beat",
    observationId: "one",
    receivedAt: 900,
    observedAt: 0,
  });
  assert.equal(clock.read(3000), 11);
});
test("beat tracking gives way immediately to pause, reverse and a different track", () => {
  for (const next of [
    {
      ...sample,
      position: 10.12,
      playing: false,
      direct: true,
      quality: "fine" as const,
    },
    {
      ...sample,
      position: 9,
      rate: -1,
      direct: true,
      quality: "fine" as const,
    },
    { ...sample, key: "b", position: 2, quality: "beat" as const },
  ]) {
    const clock = new PlaybackClock();
    clock.update({ ...sample, quality: "beat" });
    clock.read(100);
    clock.update({ ...next, receivedAt: 200, observedAt: 200 });
    assert.equal(clock.read(200), next.position);
  }
});

test("beat-motion loop wraps observation age and retains continuous forward speed", () => {
  const clock = new PlaybackClock();
  const loop = { start: 4, end: 8 };
  clock.update({
    ...sample,
    position: 7.98,
    loop,
    quality: "beat",
    observedAt: 0,
    receivedAt: 50,
  });
  assert.ok(Math.abs(clock.read(50) - 4.03) < 1e-9);
  assert.ok(Math.abs(clock.read(100) - 4.08) < 1e-9);
  clock.update({
    ...sample,
    position: 4.1,
    loop,
    quality: "beat",
    rate: 1.1,
    observedAt: 120,
    receivedAt: 150,
  });
  assert.ok(Math.abs(clock.read(150) - 4.133) < 1e-9);
  assert.ok(Math.abs(clock.read(200) - 4.188) < 1e-9);
});

test("pause and held platter freeze at observation time without delivery overshoot", () => {
  for (const playing of [false, true]) {
    const clock = new PlaybackClock();
    clock.update({
      ...sample,
      position: 10.2,
      beatNumber: 21,
      quality: "beat",
    });
    clock.read(500);
    clock.update({
      ...sample,
      position: 10,
      beatNumber: 21,
      quality: "coarse",
      direct: true,
      playing,
      receivedAt: 600,
      observedAt: 400,
    });
    assert.ok(Math.abs(clock.read(600) - 10.6) < 1e-9);
    clock.update({
      ...sample,
      position: 10,
      beatNumber: 21,
      quality: "coarse",
      direct: true,
      playing,
      receivedAt: 800,
      observedAt: 700,
    });
    assert.ok(Math.abs(clock.read(900) - 10.6) < 1e-9);
  }
});
test("fresh measured fine positions update a fractional loop immediately", () => {
  const clock = new PlaybackClock();
  const loop = { start: 4, end: 4.25 };
  clock.update({
    ...sample,
    position: 4.2,
    quality: "fine",
    loop,
    observedAt: 100,
    receivedAt: 130,
  });
  assert.ok(Math.abs(clock.read(130) - 4.23) < 1e-9);
  assert.ok(Math.abs(clock.read(160) - 4.01) < 1e-9);
  clock.update({
    ...sample,
    position: 4.12,
    quality: "fine",
    loop,
    observedAt: 200,
    receivedAt: 220,
  });
  assert.ok(Math.abs(clock.read(220) - 4.14) < 1e-9);
});

test("pause arriving after stale transport does not subtract motion that never happened", () => {
  const clock = new PlaybackClock();
  clock.update({ ...sample, position: 10.2, beatNumber: 21, quality: "beat" });
  const stopped = clock.read(3000);
  clock.update({
    ...sample,
    position: 10,
    beatNumber: 21,
    quality: "coarse",
    direct: true,
    playing: false,
    receivedAt: 5000,
    observedAt: 4900,
  });
  assert.equal(clock.read(5000), stopped);
});

test("CUE overrides a held coarse position on the same beat on its first packet", () => {
  const clock = new PlaybackClock();
  clock.update({
    key: "cue",
    position: 15.772,
    receivedAt: 0,
    playing: false,
    rate: 1,
    direct: true,
    quality: "coarse",
    beatNumber: 33,
  });
  clock.update({
    key: "cue",
    position: 15.66,
    receivedAt: 100,
    playing: false,
    rate: 1,
    direct: true,
    quality: "coarse",
    beatNumber: 33,
    cued: true,
  });
  assert.equal(clock.read(100), 15.66);
});
