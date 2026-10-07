import { test } from "node:test";
import assert from "node:assert/strict";
import { defaults, parseSettings, timeLabel } from "./settings";
test("settings recover from corrupt and outdated storage", () => {
  for (const raw of [null, "broken", "null", "[]", "42"])
    assert.deepEqual(parseSettings(raw), defaults);
  assert.deepEqual(
    parseSettings('{"window":-20,"playhead":"right","overview":"false"}'),
    defaults,
  );
  assert.deepEqual(parseSettings('{"style":"previous"}'), defaults);
  const value = {
    ...defaults,
    playhead: "center",
    time: "elapsed",
    overview: false,
    window: 16,
  };
  assert.deepEqual(parseSettings(JSON.stringify(value)), value);
});
test("time formatting clamps ended tracks and exposes missing data", () => {
  assert.equal(timeLabel(null), "—:—");
  assert.equal(timeLabel(-10), "00:00");
  assert.equal(timeLabel(380.48), "06:20");
  assert.equal(timeLabel(290.48), "04:50");
});

test("Nexus player time uses 75 frames per second with half-frame digits", async () => {
  const { playerTimeLabel } = await import("./settings");
  assert.equal(playerTimeLabel(42.389596), "00M42S29.0F");
  assert.equal(playerTimeLabel(46.907785), "00M46S68.0F");
  assert.equal(playerTimeLabel(60), "01M00S00.0F");
  assert.equal(playerTimeLabel(-1), "00M00S00.0F");
});

test("old responsive jog mode migrates to 20ms while direct and smooth remain explicit", () => {
  assert.equal(parseSettings('{"jogSmoothing":60}').jogSmoothing, 20);
  assert.equal(parseSettings('{"jogSmoothing":0}').jogSmoothing, 0);
  assert.equal(parseSettings('{"jogSmoothing":100}').jogSmoothing, 100);
});

test("BPM option ranges default to 115–130 and reject fractional or inverted bounds", () => {
  for (const range of [
    { min: 115.5, max: 130 },
    { min: 130, max: 115 },
    { min: 0, max: 130 },
    { min: 1, max: 1000 },
  ])
    assert.deepEqual(
      parseSettings(JSON.stringify({ libraryBpmRange: range })).libraryBpmRange,
      { min: 115, max: 130 },
    );
  assert.deepEqual(
    parseSettings('{"libraryBpmRange":{"min":80,"max":150}}').libraryBpmRange,
    { min: 80, max: 150 },
  );
});

test("white is the default and fine zoom steps survive storage", () => {
  assert.equal(parseSettings(null).playheadColor, "white");
  assert.equal(parseSettings('{"playheadColor":"red"}').playheadColor, "red");
  for (let window = 2; window <= 32; window += 2)
    assert.equal(parseSettings(JSON.stringify({ window })).window, window);
  for (const window of [0, 3, 34, 8.5])
    assert.equal(
      parseSettings(JSON.stringify({ window })).window,
      defaults.window,
    );
});

test("CDJ-3000 time uses colon and three fractional digits", async () => {
  const { playerTimeLabel } = await import("./settings");
  assert.equal(playerTimeLabel(102.375, true), "01:42.375");
  assert.equal(playerTimeLabel(60, true), "01:00.000");
  assert.equal(playerTimeLabel(null, true), "—:—.---");
});
