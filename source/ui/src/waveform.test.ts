import { test } from "node:test";
import assert from "node:assert/strict";
import {
  peak,
  rx3Layers,
  overviewLayers,
  viewportStart,
  sourceSpan,
} from "./waveform";
test("pooling retains a one-sample transient and zero-fills outside track", () => {
  const c = [
    { low: 0, mid: 0, high: 0 },
    { low: 1, mid: 0.5, high: 0.25 },
    { low: 0, mid: 0, high: 0 },
  ];
  assert.deepEqual(peak(c, 0, 3), c[1]);
  assert.deepEqual(peak(c, -100, -90), c[0]);
  assert.deepEqual(peak(c, 3, 4), c[0]);
});

test("native bands retain independent heights and overlap colors", () => {
  const bands = rx3Layers({ low: 0.8, mid: 0.6, high: 0.12 });
  assert.deepEqual(
    bands.map((b) => b.color),
    ["#0055e1", "#b4690a", "#f5ebd7"],
  );
  assert.equal(bands[0].height, 0.8);
  assert.ok(Math.abs(bands[1].height - 0.6) < 1e-10);
  assert.equal(bands[2].height, 0.12);
  assert.deepEqual(rx3Layers({ low: 0, mid: 0, high: 0 }), []);
  assert.deepEqual(rx3Layers({ low: 0, mid: 1, high: 0 }), [
    { height: 1, color: "#ffa600" },
  ]);
  assert.deepEqual(rx3Layers({ low: 0, mid: 0, high: 1 }), [
    { height: 1, color: "#ffffff" },
  ]);
});
test("overview uses stacked contributions and shared track scaling", () => {
  const bands = overviewLayers({ low: 1, mid: 1, high: 1 }, 1.06);
  assert.equal(bands[0].height, 1);
  assert.equal(bands[2].height, 0.49 / 1.06);
  assert.deepEqual(
    bands.map((b) => b.color),
    ["#ffffff", "#ffa600", "#0055e1"],
  );
  assert.deepEqual(overviewLayers({ low: 0, mid: 0, high: 0 }, 0), []);
});

test("playhead anchor maps the current sample to one third or the center", () => {
  for (const anchor of [1 / 3, 0.5]) {
    const start = viewportStart(90, 150, 8, anchor);
    assert.equal(start + 8 * 150 * anchor, 90 * 150);
  }
});
test("synced 122 and 120 BPM tracks have matching beat spacing at 121.41 BPM", () => {
  for (const zoom of [2, 14, 32]) {
    for (const anchor of [1 / 3, 0.5]) {
      const positions = [99.431, 9.655];
      const xs = [122, 120].map((bpm, i) => {
        const span = sourceSpan(zoom, 121.41 / bpm);
        const start = viewportStart(positions[i], 150, span, anchor) / 150;
        // Corresponding beats ahead of a phase-aligned playhead must line up.
        return [0, 1, 4, 16].map(
          (beat) => ((positions[i] + (beat * 60) / bpm - start) / span) * 1000,
        );
      });
      xs[0].forEach((x, i) => assert.ok(Math.abs(x - xs[1][i]) < 1e-9));
    }
  }
});
test("waveform scale preserves reverse speed and falls back for missing or invalid rates", () => {
  assert.equal(sourceSpan(14, -1.2), sourceSpan(14, 1.2));
  for (const rate of [undefined, 0, NaN, Infinity])
    assert.equal(sourceSpan(14, rate), 14);
});
test("optional bass lift never adds absent bass or changes other bands", () => {
  assert.deepEqual(
    rx3Layers({ low: 0, mid: 1, high: 0.5 }, true),
    rx3Layers({ low: 0, mid: 1, high: 0.5 }, false),
  );
  assert.equal(rx3Layers({ low: 0.5, mid: 0, high: 0 }, true)[0].height, 0.575);
});
