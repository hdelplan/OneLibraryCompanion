import test from "node:test";
import assert from "node:assert/strict";
import {
  activeFilterParams,
  clearTrackFilters,
  selectedLibrarySource,
  type LibrarySource,
  folderPath,
  selection,
  filterCount,
  type Playlist,
} from "./libraryModel";
test("malformed playlist cycles terminate and preserve useful breadcrumbs", () => {
  const nodes: Playlist[] = [
    { id: 1, parentId: 2, name: "One", folder: true, order: 0, count: 0 },
    { id: 2, parentId: 1, name: "Two", folder: true, order: 0, count: 0 },
  ];
  assert.deepEqual(
    folderPath(nodes, 1).map((n) => n.id),
    [2, 1],
  );
  assert.deepEqual(folderPath(nodes, 99), []);
});
test("empty colors remain selectable and empty multi-selections are not active filters", () => {
  assert.deepEqual(selection({ color: '["","Blue"]' }, "color"), ["", "Blue"]);
  assert.deepEqual(selection({ color: '{"bad":true}' }, "color"), []);
  assert.equal(
    filterCount({ color: "[]", rating: "4", sort: "title", tagMode: "all" }),
    1,
  );
});

import {
  categoryKey,
  defaultLibraryFilters,
  displayedFilters,
  moveFilter,
  nextColumnSort,
  parseLibraryFilters,
} from "./libraryPreferences";
test("columns start ascending, toggle descending, and reset direction for another column", () => {
  const first = nextColumnSort({ sort: "playlist" }, "bpm");
  assert.equal(first.direction, "asc");
  assert.equal(nextColumnSort(first, "bpm").direction, "desc");
  assert.equal(
    nextColumnSort({ ...first, direction: "desc" }, "genre").direction,
    "asc",
  );
});
test("custom category names resolve without hardcoded labels and retain configured order", () => {
  const categories = ["Genre", "Venue / Room", "Situation ✨"];
  assert.deepEqual(displayedFilters(["color", "tag:*", "rating"], categories), [
    "color",
    ...categories.map(categoryKey),
    "rating",
  ]);
  assert.deepEqual(
    moveFilter(
      ["color", "rating", categoryKey("Venue / Room")],
      categoryKey("Venue / Room"),
      -1,
    ),
    ["color", categoryKey("Venue / Room"), "rating"],
  );
  assert.deepEqual(
    parseLibraryFilters([
      "album",
      "bogus",
      "color",
      "color",
      categoryKey("Situation ✨"),
    ]),
    ["color", categoryKey("Situation ✨")],
  );
  assert.ok(!defaultLibraryFilters.includes("album"));
});

test("position-only status updates do not invalidate the library; load safety transitions do", async () => {
  const { sameLoadPlayers, loadBlocked } = await import("./libraryLoading");
  const player = {
    number: 1,
    connection: "connected",
    playing: false,
    playState: "paused",
    position: 10,
  } as import("./model").LivePlayer;
  assert.equal(
    sameLoadPlayers([player], [{ ...player, position: 10.5, statusAgeMs: 20 }]),
    true,
  );
  assert.equal(
    sameLoadPlayers([player], [{ ...player, playing: true }]),
    false,
  );
  assert.equal(
    sameLoadPlayers([player], [{ ...player, connection: "stale" }]),
    false,
  );
  for (const playState of [
    "playing",
    "looping",
    "cue play",
    "cue scratch",
    "searching",
    "emergency loop",
  ])
    assert.equal(loadBlocked({ ...player, playState }), true);
  assert.equal(loadBlocked(player), false);
  assert.equal(
    loadBlocked({ ...player, playState: "looping", playing: false }),
    true,
  );
});

test("cleared chips and saved empty filters are omitted while missing-value selections remain", () => {
  const filters = {
    genre: "",
    color: '[""]',
    key: "[]",
    "tagCategory:2": '["20"]',
    sort: "key",
  };
  assert.deepEqual(activeFilterParams(filters), {
    color: '[""]',
    "tagCategory:2": '["20"]',
    sort: "key",
  });
  assert.equal(filterCount(activeFilterParams(filters)), 2);
});

test("USB selection handles zero, one and two mounted sources without losing the user's choice", () => {
  const first: LibrarySource = {
    id: "direct:192.0.2.1",
    label: "CDJ 1 USB",
    direct: true,
    available: true,
    loadable: false,
    generation: 1,
    state: "ready",
    error: null,
    count: 1,
  };
  const second = { ...first, id: "direct:192.0.2.2", label: "CDJ 2 USB" };
  assert.equal(selectedLibrarySource("", []), "");
  assert.equal(
    selectedLibrarySource(first.id, [{ ...first, available: false }]),
    "",
  );
  assert.equal(selectedLibrarySource("", [second]), second.id);
  assert.equal(
    selectedLibrarySource(first.id, [{ ...first, available: false }, second]),
    second.id,
  );
  assert.equal(selectedLibrarySource(second.id, [first, second]), second.id);
  assert.equal(selectedLibrarySource("", [first, second]), first.id);
});

test("clearing track filters preserves collection selection and ordering", () => {
  const navigation = {
    playlist: "42",
    set: "saved-set",
    sort: "bpm",
    direction: "desc",
    tagMode: "all",
  };
  const cleared = clearTrackFilters({
    ...navigation,
    rating: "4",
    genre: '["House"]',
    bpmMin: "120",
    q: "track",
  });
  assert.deepEqual(cleared, navigation);
  assert.equal(filterCount(cleared), 0);
});
