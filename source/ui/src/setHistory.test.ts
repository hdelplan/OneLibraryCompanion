import test from "node:test";
import assert from "node:assert/strict";
import {
  type DJSet,
  orderedTracks,
  setCsv,
  setText,
  playedTrackKeys,
  playedTrackKey,
} from "./setHistoryModel";
const set: DJSet = {
  id: "one",
  title: "Sample",
  startedAt: 1772323200000,
  endedAt: 1772333200000,
  dateOnly: false,
  location: "Venue, Paris",
  comment: 'A "special"\nnight',
  origin: "sample",
  recovered: false,
  events: [
    {
      id: "a",
      deck: 1,
      source: "usb",
      track: {
        id: 1,
        title: '=HYPERLINK("bad")',
        artist: "Björk",
        key: "8A",
        rating: 4,
        artwork: null,
      },
    },
    {
      id: "b",
      deck: 2,
      source: "usb",
      track: {
        id: 2,
        title: "Second",
        artist: "Artist",
        key: "9A",
        rating: 5,
        artwork: null,
      },
    },
  ],
  order: ["b", "a"],
};
test("exports follow edited order and keep original events", () => {
  assert.equal(orderedTracks(set)[0].id, "b");
  assert.equal(set.events[0].id, "a");
  assert.match(setText([set]), /1\. Second/);
  assert.match(setText([set]), /SAMPLE/);
  assert.match(setText([set]), /Björk/);
  assert.match(setText([{ ...set, order: ["a"] }]), /1\. =HYPERLINK/);
});
test("CSV preserves commas quotes Unicode and multiline fields and escapes formulas", () => {
  const csv = setCsv([set]);
  assert.ok(csv.startsWith("\uFEFF"));
  assert.ok(csv.includes('"Venue, Paris"'));
  assert.ok(csv.includes('"A ""special""\nnight"'));
  assert.ok(csv.includes("\"'=HYPERLINK"));
  assert.ok(csv.includes("Björk"));
  assert.ok(csv.indexOf("Second") < csv.indexOf("HYPERLINK"));
});
test("all-set exports include every set and empty sets retain metadata", () => {
  assert.match(setCsv([{ ...set, order: [] }]), /Venue, Paris/);
  const text = setText([set, { ...set, id: "two", title: "Other", order: [] }]);
  assert.match(text, /SET HISTORY — Other/);
  assert.match(text, /SET HISTORY — Sample/);
});

test("played coloring is independent of set recording and distinguishes paths with reused IDs", () => {
  const track = {
    id: 1,
    title: "Track",
    artist: "Artist",
    filePath: "/Contents/a.mp3",
    key: "8A",
    rating: 3,
    artwork: null,
  };
  const current = {
    ...set,
    events: [{ id: "played", deck: 1, source: "usb", track }],
    order: ["played"],
  };
  const history = {
    version: 1,
    revision: 1,
    sets: [current],
    activeId: current.id,
    recording: true,
    error: null,
    importNote: "",
    pending: [],
    playedTracks: [track],
  };
  assert.ok(playedTrackKeys(history).has(playedTrackKey(track)!));
  assert.ok(
    !playedTrackKeys(history).has(
      playedTrackKey({ ...track, filePath: "/Contents/b.mp3" })!,
    ),
  );
  assert.equal(
    playedTrackKeys({ ...history, activeId: null, recording: false }).size,
    1,
  );
  assert.equal(playedTrackKey({ ...track, filePath: undefined }), null);
  assert.equal(
    playedTrackKeys({ ...history, sets: [{ ...current, order: [] }] }).size,
    1,
  );
});
