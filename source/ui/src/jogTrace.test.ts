import { test } from "node:test";
import assert from "node:assert/strict";
import {
  startJogTrace,
  recordJogFrame,
  recordJogSnapshot,
  stopJogTrace,
} from "./jogTrace";
test("capture uses its own monotonic clock even with an offset animation timestamp", () => {
  startJogTrace();
  recordJogSnapshot([{ number: 1 }]);
  recordJogFrame(
    {
      key: "track",
      position: 1,
      receivedAt: performance.now(),
      playing: true,
      rate: 1,
    },
    1,
    1e12,
    16,
    0,
    1,
  );
  const report = stopJogTrace();
  assert.equal(report.frames.length, 1);
  assert.equal(report.snapshots.length, 1);
  assert.deepEqual(report.warnings, []);
});
test("capture explains missing live motion instead of silently exporting an empty array", () => {
  startJogTrace();
  recordJogFrame(undefined, null, 0, 16, 0, 0);
  const report = stopJogTrace();
  assert.equal(report.missingMotion, 1);
  assert.match(report.warnings[0], /no live motion/);
});
test("host capture session starts a browser recorder that missed the button and does not reset on heartbeat", async () => {
  const { syncJogTrace } = await import("./jogTrace");
  syncJogTrace({ active: true, session: "host-test", remainingMs: 40000 });
  recordJogFrame(
    {
      key: "track",
      position: 1,
      receivedAt: performance.now(),
      playing: true,
      rate: 1,
    },
    1,
    0,
    16,
    0,
    1,
  );
  syncJogTrace({ active: true, session: "host-test", remainingMs: 39000 });
  const report = stopJogTrace();
  assert.equal(report.hostSession, "host-test");
  assert.equal(report.frames.length, 1);
});
test("automatic cue windows retain separate browser reports across host sessions", async () => {
  const { syncJogTrace, cueBrowserReports, clearCueBrowserReports } =
    await import("./jogTrace");
  clearCueBrowserReports();
  for (const session of ["cue-a", "cue-b"]) {
    syncJogTrace({
      active: true,
      session,
      profile: "cue-window",
      remainingMs: 20000,
    });
    recordJogSnapshot([{ number: 2, session }]);
    syncJogTrace({ active: false, session, profile: "cue-window" });
  }
  const reports = cueBrowserReports();
  assert.deepEqual(
    reports.map((r) => r.hostSession),
    ["cue-a", "cue-b"],
  );
  assert.equal(reports[0].snapshots.length, 1);
  assert.equal(reports[1].snapshots.length, 1);
  assert.equal(cueBrowserReports().length, 2);
  clearCueBrowserReports();
  assert.equal(cueBrowserReports().length, 0);
});
