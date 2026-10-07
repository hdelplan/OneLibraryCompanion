import { test } from "node:test";
import assert from "node:assert/strict";
import { cueCountdown } from "./cueCountdownModel";
const beats = Array.from({ length: 65 }, (_, i) => ({
  time: i * 0.5,
  beatInBar: (i % 4) + 1,
}));
const cue = (time: number, hot = 1) => ({
  time,
  hot,
  label: "A",
  color: "#fff",
});
test("countdown counts bars and beats to the next hot cue, ignoring memory cues", () => {
  assert.equal(cueCountdown(0, beats, [cue(2, 0), cue(11)]).text, "05.2");
  assert.equal(cueCountdown(10.6, beats, [cue(11)]).text, "00.1");
  assert.equal(cueCountdown(11, beats, [cue(11)]).text, "00.0");
  assert.equal(cueCountdown(11.01, beats, [cue(11)]).text, "—.—");
  assert.equal(cueCountdown(null, beats, [cue(11)]).text, "—.—");
  assert.equal(cueCountdown(0, undefined, [cue(11)]).text, "—.—");
});
test("countdown follows tempo changes, seeks and fractional cue positions", () => {
  const varied = beats.map((b, i) => ({
    ...b,
    time: i <= 4 ? i * 0.5 : 2 + (i - 4),
  }));
  assert.equal(cueCountdown(0, varied, [cue(6)]).text, "02.0");
  assert.equal(cueCountdown(2, varied, [cue(6)]).text, "01.0");
  assert.equal(cueCountdown(2.25, varied, [cue(6.25)]).text, "01.0");
  assert.equal(cueCountdown(0, varied, [cue(6), cue(2)]).text, "01.0");
});
