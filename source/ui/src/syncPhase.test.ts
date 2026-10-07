import test from "node:test";
import assert from "node:assert/strict";
import { beatPhase, phaseDistance, PhaseMismatch } from "./syncPhase";
test("phase compares fractional beats across boundaries and different tempos", () => {
  const beats = [0, 0.5, 1].map((time) => ({ time, beatInBar: 1 }));
  assert.equal(beatPhase(0.25, beats), 0.5);
  assert.equal(beatPhase(1, beats), null);
  assert.ok(Math.abs(phaseDistance(0.98, 0.02) - 0.04) < 1e-10);
});
test("phase flashing requires persistent error, hysteresis and valid data", () => {
  const detector = new PhaseMismatch();
  assert.equal(detector.update(0.08, 0), false);
  assert.equal(detector.update(0.08, 250), true);
  assert.equal(detector.update(0.04, 600), true);
  assert.equal(detector.update(0.01, 700), true);
  assert.equal(detector.update(0.01, 950), false);
  detector.update(0.1, 1000);
  assert.equal(detector.update(0.1, 1300), true);
  assert.equal(detector.update(null, 1350), false);
});
