import { test } from "node:test";
import assert from "node:assert/strict";
import { bandHeight } from "./bandGeometry";
test("native envelope preserves relative heights without inventing signal", () => {
  assert.equal(bandHeight(0.25), 0.25);
  assert.equal(bandHeight(0), 0);
  assert.equal(bandHeight(NaN), 0);
  assert.equal(bandHeight(4), 1);
});
