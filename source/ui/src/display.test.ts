import { test } from "node:test";
import assert from "node:assert/strict";
import { displayLayout, resolutions } from "./display";
import { parseSettings } from "./settings";
test("resolution preferences migrate and persist", () => {
  for (const resolution of ["auto", ...Object.keys(resolutions)])
    assert.equal(
      parseSettings(JSON.stringify({ resolution })).resolution,
      resolution,
    );
  for (const resolution of [undefined, "invalid", "__proto__", 1920, null])
    assert.equal(
      parseSettings(JSON.stringify({ resolution })).resolution,
      "auto",
    );
});
test("layouts fit all viewports and preserve the selected ratio", () => {
  for (const resolution of ["auto", ...Object.keys(resolutions)] as const) {
    for (const [w, h] of [
      [1280, 800],
      [1920, 720],
      [1920, 1080],
      [1024, 768],
      [768, 1024],
    ]) {
      const mode = resolution as "auto" | keyof typeof resolutions;
      const layout = displayLayout(mode, w, h);
      const [rw, rh] = mode === "auto" ? [w, h] : resolutions[mode];
      assert.ok(Math.abs(layout.width / layout.height - rw / rh) < 1e-9);
      assert.ok(layout.width * layout.scale <= w + 1e-9);
      assert.ok(layout.height * layout.scale <= h + 1e-9);
      if (mode === "auto") {
        assert.ok(Math.abs(layout.width * layout.scale - w) < 1e-9);
        assert.ok(Math.abs(layout.height * layout.scale - h) < 1e-9);
      }
    }
  }
  assert.equal(displayLayout("1280x800", 1280, 800).width, 1280);
  assert.ok(displayLayout("1920x720", 1920, 720).width > 2100);
});
