import test from "node:test";
import assert from "node:assert/strict";
import {
  newlyAvailableLocalSource,
  selectedLibrarySource,
  type LibrarySource,
} from "./libraryModel";
const source = (id: string, available = true): LibrarySource => ({
  id,
  available,
  label: id,
  loadable: false,
  generation: 1,
  state: "ready",
  error: null,
  count: 2,
});
test("new local USB selects once; normal polling preserves deliberate selection", () => {
  const cdj = source("direct:cdj1"),
    usb = source("local-usb:one");
  assert.equal(newlyAvailableLocalSource([cdj], [cdj, usb]), usb.id);
  assert.equal(newlyAvailableLocalSource([cdj, usb], [cdj, usb]), undefined);
  assert.equal(selectedLibrarySource(cdj.id, [cdj, usb]), cdj.id);
});
test("three local libraries coexist; removal falls back and reconnection is noticed", () => {
  const volumes = ["one", "two", "three"].map((id) =>
    source(`local-usb:${id}`),
  );
  assert.equal(selectedLibrarySource(volumes[2].id, volumes), volumes[2].id);
  const removed = [{ ...volumes[0], available: false }, ...volumes.slice(1)];
  assert.equal(selectedLibrarySource(volumes[0].id, removed), volumes[1].id);
  assert.equal(newlyAvailableLocalSource(removed, volumes), volumes[0].id);
});
