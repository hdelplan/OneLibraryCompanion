import assert from "node:assert/strict";
import test from "node:test";
import { incompatiblePlayers, unsupportedReason } from "./trackCompatibility";
import { sameLoadPlayers } from "./libraryLoading";
import type { LibraryTrack } from "./libraryModel";
import type { LivePlayer } from "./model";
const track = (format: string, sampleRate = 44100, sampleDepth = 16) =>
  ({ format, sampleRate, sampleDepth }) as unknown as LibraryTrack;
const player = (name: string, number = 1) =>
  ({
    name,
    number,
    connection: "connected",
    playState: "paused",
  }) as LivePlayer;
test("mixed decks warn only for the incompatible target", () => {
  const decks = [player("CDJ-2000nexus"), player("CDJ-2000NXS2", 2)];
  assert.deepEqual(incompatiblePlayers(track("flac"), decks), [decks[0]]);
  assert.ok(unsupportedReason(track("alac"), decks[0]));
  assert.equal(unsupportedReason(track("mp3"), decks[0]), null);
});
test("sample rates and lossless bit depths follow each model", () => {
  for (const name of ["CDJ-2000NXS", "CDJ-2000nexus"]) {
    assert.ok(unsupportedReason(track("wav", 96000), player(name)));
    assert.equal(
      unsupportedReason(track("aiff", 48000, 24), player(name)),
      null,
    );
  }
  for (const name of ["CDJ-2000NXS2", "CDJ-3000"]) {
    assert.equal(
      unsupportedReason(track("flac", 96000, 24), player(name)),
      null,
    );
    assert.ok(unsupportedReason(track("wav", 192000), player(name)));
    assert.ok(unsupportedReason(track("wav", 48000, 32), player(name)));
  }
  assert.equal(
    unsupportedReason(track("mp3", 32000), player("CDJ-2000NXS2")),
    null,
  );
  assert.ok(unsupportedReason(track("mp3", 32000), player("CDJ-3000")));
});
test("unknown metadata, unknown models and absent decks are inconclusive", () => {
  assert.equal(
    unsupportedReason(track("Container(99)"), player("CDJ-2000nexus")),
    null,
  );
  assert.equal(
    unsupportedReason(track("wav", 0, 0), player("CDJ-2000nexus")),
    null,
  );
  assert.equal(unsupportedReason(track("flac"), player("CDJ-3000X")), null);
  assert.equal(unsupportedReason(track("flac"), undefined), null);
  for (const connection of ["stale", "disconnected"] as const)
    assert.equal(
      unsupportedReason(track("flac"), {
        ...player("CDJ-2000nexus"),
        connection,
      }),
      null,
    );
});
test("model changes refresh memoized rows", () => {
  assert.equal(
    sameLoadPlayers([player("CDJ-2000nexus")], [player("CDJ-2000NXS2")]),
    false,
  );
});
