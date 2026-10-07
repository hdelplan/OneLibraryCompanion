import { test } from "node:test";
import assert from "node:assert/strict";
import { activeMixKey, camelotKey, keyCompatibility } from "./keyCompatibility";
import { liveDeck } from "./useLiveDecks";
import type { LivePlayer } from "./model";

test("requested fuzzy matches use green before semi-compatible pink", () => {
  for (const key of ["3A", "3B", "2A", "4A"])
    assert.equal(keyCompatibility("3A", key), "compatible", key);
  for (const key of ["5A", "4B", "5B", "10B", "10A", "2B"])
    assert.equal(keyCompatibility("3A", key), "semi", key);
  assert.equal(keyCompatibility("3A", "6A"), null);
  assert.equal(keyCompatibility("12A", "1A"), "compatible");
  assert.equal(keyCompatibility("12A", "2B"), "semi");
  assert.equal(keyCompatibility("12A", "2A"), "semi");
  assert.equal(keyCompatibility("3B", "5B"), "semi");
  assert.equal(keyCompatibility(null, "3A"), null);
  assert.equal(keyCompatibility("3A", "unknown"), null);
});
test("traditional, enharmonic and Open Key names map to Camelot", () => {
  for (const key of ["Bbm", "A♯ minor", "B Flat Minor", "8m", "3a"])
    assert.deepEqual(camelotKey(key), { number: 3, mode: "A" });
  assert.deepEqual(camelotKey("CM"), camelotKey("8B"));
  assert.deepEqual(camelotKey("1m"), camelotKey("8A"));
  for (const key of ["", "13A", "0A", "unknown"])
    assert.equal(camelotKey(key), null);
});
test("highlight reference follows only the sole connected playing deck", () => {
  const player = {
    number: 1,
    connection: "connected",
    playing: true,
    position: 0,
    trackKey: "a",
  } as LivePlayer;
  const first = liveDeck(player)!;
  first.analysis = {
    detail: null,
    preview: null,
    track: { key: "3A" } as NonNullable<typeof first.analysis.track>,
  };
  const second = liveDeck({ ...player, number: 2, playing: false })!;
  second.analysis = {
    ...first.analysis,
    track: { ...first.analysis.track!, key: "8B" },
  };
  assert.equal(activeMixKey([first, second]), "3A");
  second.live!.playing = true;
  assert.equal(activeMixKey([first, second]), null);
  first.live!.playing = false;
  assert.equal(activeMixKey([first, second]), "8B");
  second.live!.playing = false;
  assert.equal(activeMixKey([first, second]), null);
  second.live!.playing = true;
  second.live!.connection = "stale";
  assert.equal(activeMixKey([first, second]), null);
});
