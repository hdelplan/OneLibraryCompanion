import { test } from "node:test";
import assert from "node:assert/strict";
import { beatAt, comparisonReady } from "./phaseMeter";
import type { Deck } from "./model";
import { defaults, parseSettings } from "./settings";

test("phase follows variable beatgrid intervals and rejects missing coverage", () => {
  const grid = [
    { time: 0, beatInBar: 1 },
    { time: 0.5, beatInBar: 2 },
    { time: 1.1, beatInBar: 3 },
  ];
  assert.deepEqual(beatAt(0.25, grid), {
    index: 0,
    fraction: 0.5,
    period: 0.5,
  });
  const second = beatAt(0.8, grid)!;
  assert.equal(second.index, 1);
  assert.ok(Math.abs(second.fraction - 0.5) < 1e-9);
  for (const value of [-1, 1.1, 2, NaN])
    assert.equal(beatAt(value, grid), null);
  assert.equal(beatAt(0, []), null);
});

test("comparison rejects stale, coarse, disconnected and manual movement", () => {
  const deck = {
    source: "live",
    position: 2,
    analysis: { detail: null, preview: null },
    motion: {
      key: "track",
      position: 2,
      receivedAt: 100,
      observedAt: 100,
      playing: true,
      rate: 1,
      quality: "fine",
    },
    live: { connection: "connected" },
  } as Deck;
  assert.equal(comparisonReady(deck, 500), true);
  assert.equal(comparisonReady(deck, 1200), false);
  assert.equal(comparisonReady(null, 500), false);
  for (const changes of [
    { direct: true },
    { playing: false },
    { quality: "coarse" },
    { quality: "held" },
  ])
    assert.equal(
      comparisonReady(
        { ...deck!, motion: { ...deck!.motion!, ...changes } } as Deck,
        500,
      ),
      false,
    );
  assert.equal(
    comparisonReady(
      { ...deck!, live: { ...deck!.live!, connection: "disconnected" } },
      500,
    ),
    false,
  );
});

test("wide layout and comparison settings migrate and preserve choices", () => {
  assert.equal(parseSettings("{}").wideLayout, "side-by-side");
  assert.equal(parseSettings("{}").phaseDisplay, "phase");
  const changed = {
    ...defaults,
    wideLayout: "stacked",
    phaseDisplay: "waveform",
  };
  assert.deepEqual(parseSettings(JSON.stringify(changed)), changed);
  const invalid = parseSettings('{"wideLayout":"invalid","phaseDisplay":null}');
  assert.equal(invalid.wideLayout, defaults.wideLayout);
  assert.equal(invalid.phaseDisplay, defaults.phaseDisplay);
});
