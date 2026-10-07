import { test } from "node:test";
import assert from "node:assert/strict";
import {
  handoffPrompt,
  syncTapPrompt,
  type CaptureState,
} from "./handoffCapture";
const state = (remainingMs: number, master: number): CaptureState => ({
  active: true,
  session: "test",
  remainingMs,
  players: [1, 2].map((number) => ({
    number,
    master: number === master,
    masterMeaningful: number === master,
    yieldingTo: null,
  })),
});
test("handoff guide waits for the actual master and records a post-return phase", () => {
  assert.match(handoffPrompt(state(45000, 1)), /Keep CDJ1 master/);
  assert.match(handoffPrompt(state(33000, 1)), /Press CDJ2 MASTER/);
  assert.match(handoffPrompt(state(30000, 2)), /CDJ2 master confirmed/);
  assert.match(handoffPrompt(state(20000, 2)), /Press CDJ1 MASTER now/);
  assert.match(
    handoffPrompt(state(15000, 1)),
    /test whether fine updates persist/,
  );
  const overlap = state(30000, 2);
  overlap.players![0].master = true;
  assert.doesNotMatch(handoffPrompt(overlap), /master confirmed/);
  assert.match(
    handoffPrompt({
      ...state(30000, 2),
      active: false,
      message: "Sync enabled. Restore CDJ1.",
    }),
    /Sync enabled/,
  );
});

test("Sync guide separates slow control, three quick attempts and reset intervals", () => {
  assert.match(syncTapPrompt(state(45000, 1)), /Wait with CDJ2 Sync OFF/);
  assert.match(syncTapPrompt(state(40000, 1)), /Slow control/);
  assert.match(syncTapPrompt(state(35000, 1)), /switch it OFF/);
  assert.match(syncTapPrompt(state(30000, 1)), /Quick attempt 1/);
  assert.match(syncTapPrompt(state(25000, 1)), /Wait/);
  assert.match(syncTapPrompt(state(20000, 1)), /Quick attempt 2/);
  assert.match(syncTapPrompt(state(15000, 1)), /Wait/);
  assert.match(syncTapPrompt(state(10000, 1)), /Quick attempt 3/);
  assert.match(syncTapPrompt(state(5000, 1)), /Ensure CDJ2 Sync is OFF/);
  assert.match(
    syncTapPrompt({
      ...state(45000, 1),
      active: false,
      message: "CDJ1 Sync enabled; stopped.",
    }),
    /CDJ1 Sync enabled/,
  );
});
