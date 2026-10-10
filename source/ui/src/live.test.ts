import { test } from "node:test";
import assert from "node:assert/strict";
import { liveDeck, ObservationTimes, playerSlots } from "./useLiveDecks";
import type { LivePlayer, Analysis } from "./model";
const player: LivePlayer = {
  number: 1,
  name: "CDJ",
  ip: "192.0.2.1",
  connection: "connected",
  playState: "playing",
  bpm: 128,
  pitch: 0,
  master: true,
  sync: false,
  trackKey: "first",
  assetReady: true,
  position: 90,
  warning: null,
  sourceLabel: "USB",
};
const analysis: Analysis = {
  detail: null,
  preview: null,
  track: {
    id: 1,
    title: "Synthetic",
    artist: "",
    album: "",
    key: "",
    bpm: 128,
    duration: 180,
    genre: "",
    bitrate: 0,
    sampleRate: 0,
  },
};
test("fine bar positions use direct motion even while the deck reports playing", () => {
  const deck = liveDeck({ ...player, positionSource: "bar-phase" })!;
  assert.equal(deck.motion!.direct, true);
  assert.equal(deck.motion!.playing, true);
  assert.equal(
    liveDeck({ ...player, positionSource: "beat-grid-estimate" })!.motion!
      .direct,
    false,
  );
});
test("disconnect and stale status clear metadata, time and live measurements", () => {
  for (const connection of ["stale", "disconnected"] as const) {
    const deck = liveDeck({ ...player, connection }, analysis)!;
    assert.equal(deck.position, null);
    assert.equal(deck.analysis.track, null);
    assert.equal(deck.live!.bpm, null);
    assert.equal(deck.live!.master, null);
    assert.equal(deck.live!.pitch, null);
    assert.equal(deck.live!.playState, null);
  }
});
test("new track with analysis pending cannot inherit the previous track", () => {
  const first = liveDeck(player, analysis)!;
  assert.equal(first.analysis.track!.id, 1);
  const next = liveDeck({
    ...player,
    trackKey: "second",
    assetReady: false,
    position: null,
  })!;
  assert.equal(next.analysis.track, null);
  assert.equal(next.position, null);
  assert.equal(next.live!.playState, "playing");
  assert.equal(next.live!.bpm, 128);
});

test("fine positions during automated short loops do not enable manual-jog mode", () => {
  const deck = liveDeck({
    ...player,
    positionSource: "bar-phase",
    manualMotion: false,
    playState: "looping",
  })!;
  assert.equal(deck.motion!.direct, false);
  const jog = liveDeck({
    ...player,
    positionSource: "bar-phase",
    manualMotion: true,
  })!;
  assert.equal(jog.motion!.direct, true);
});
test("direct IP beat-only positions are not extrapolated as precise playback", () => {
  const deck = liveDeck({ ...player, positionSource: "direct-status-beat" })!;
  assert.equal(deck.motion!.direct, true);
});
test("ambiguous short-loop positions stay direct without claiming reverse or old bounds", async () => {
  const { PlaybackClock } = await import("./playbackClock");
  const deck = liveDeck({
    ...player,
    playing: true,
    playState: "looping",
    positionSource: "status-beat-estimate",
    positionQuality: "coarse",
    manualMotion: true,
    reverse: false,
    loop: null,
  })!;
  assert.equal(deck.motion!.direct, true);
  assert.equal(deck.motion!.loop, undefined);
  assert.equal(deck.motion!.rate, 1);
  const clock = new PlaybackClock();
  clock.update({ ...deck.motion!, receivedAt: 0 });
  assert.equal(clock.read(500), player.position);
});
test("direct-IP live transport advances between status beats and stops on pause", async () => {
  const { PlaybackClock } = await import("./playbackClock");
  const clock = new PlaybackClock();
  const moving = liveDeck({
    ...player,
    sourceLabel: "Direct IP · USB",
    positionSource: "beat-grid-estimate",
    playing: true,
  })!;
  clock.update({ ...moving.motion!, receivedAt: 0 });
  assert.equal(clock.read(250), 90.25);
  const paused = liveDeck({
    ...player,
    sourceLabel: "Direct IP · USB",
    positionSource: "bar-phase",
    playing: false,
    manualMotion: true,
    playState: "paused",
    position: 90.3,
  })!;
  clock.update({ ...paused.motion!, receivedAt: 300 });
  assert.equal(clock.read(800), 90.3);
});

test("manual movement stays direct when fine phase is unavailable", () => {
  for (const positionQuality of ["fine", "held", "coarse"] as const) {
    const deck = liveDeck({
      ...player,
      manualMotion: true,
      playing: true,
      positionSource:
        positionQuality === "coarse" ? "beat-grid-estimate" : "bar-phase",
      positionQuality,
    });
    assert.equal(deck!.motion!.direct, true);
  }
});
test("synced non-master deck scrolls between coarse beat packets", async () => {
  const { PlaybackClock } = await import("./playbackClock");
  const clock = new PlaybackClock();
  const deck = liveDeck(
    {
      ...player,
      number: 2,
      master: false,
      sync: true,
      playing: true,
      manualMotion: false,
      positionQuality: "coarse",
      positionSource: "status-beat-estimate",
      beatNumber: 21,
      pitch: 1.18,
    },
    analysis,
    0,
  )!;
  clock.update(deck.motion!);
  assert.equal(deck.motion!.direct, false);
  assert.equal(deck.motion!.beatOnly, true);
  const a = clock.read(16);
  const b = clock.read(32);
  assert.ok(a > 90 && b > a);
  assert.ok(Math.abs(b - 90.0323776) < 1e-9);
});
test("observation age survives repeated host snapshots and resets for a new packet", () => {
  const times = new ObservationTimes();
  const first = { ...player, observationId: "session:1", statusAgeMs: 25 };
  assert.equal(times.resolve(first, 100), 75);
  // A replayed heartbeat can even repeat its old age; it must not renew freshness.
  assert.equal(times.resolve(first, 900), 75);
  assert.equal(
    times.resolve(
      { ...first, observationId: "session:2", statusAgeMs: 10 },
      1000,
    ),
    990,
  );
  const deck = liveDeck(first, analysis, 100, 75)!;
  assert.equal(deck.motion!.observedAt, 75);
  assert.equal(deck.motion!.observationId, "session:1");
});

test("CDJ 2 keeps the second display slot when CDJ 1 disconnects", () => {
  const second = { ...player, number: 2, ip: "192.0.2.2" };
  assert.deepEqual(playerSlots([second]), [undefined, second]);
  assert.deepEqual(playerSlots([second, player]), [player, second]);
  assert.deepEqual(playerSlots([player]), [player, undefined]);
});
test("other desktop player numbers still occupy available display slots", () => {
  const third = { ...player, number: 3 };
  const fourth = { ...player, number: 4 };
  assert.deepEqual(playerSlots([fourth, third]), [third, fourth]);
});

test("both decks use motion speed and position age without changing tempo display", () => {
  for (const master of [false, true]) {
    const input: LivePlayer = {
      ...player,
      master,
      positionSource: "beat-motion",
      positionQuality: "beat",
      manualMotion: false,
      motionRate: 0.94,
      pitch: -3,
      positionAgeMs: 8,
      statusAgeMs: 100,
      observationId: "beat1",
    };
    const times = new ObservationTimes();
    assert.equal(times.resolve(input, 200), 192);
    assert.equal(times.resolve({ ...input, positionAgeMs: 18 }, 210), 192);
    const deck = liveDeck(input, analysis, 200)!;
    assert.equal(deck.motion!.observedAt, 192);
    assert.equal(deck.motion!.rate, 0.94);
    assert.equal(deck.motion!.tempoRate, 0.97);
    assert.equal(deck.motion!.beatOnly, false);
    assert.equal(deck.motion!.direct, false);
    assert.equal(deck.live!.pitch, -3);
  }
});

test("relative phase positions retain measured motion speed and timestamp", () => {
  const deck = liveDeck(
    {
      ...player,
      master: false,
      positionSource: "relative-phase",
      positionQuality: "beat",
      motionRate: 0.94,
      pitch: -3,
      positionAgeMs: 20,
      manualMotion: false,
    },
    analysis,
    200,
  )!;
  assert.equal(deck.motion!.rate, 0.94);
  assert.equal(deck.motion!.observedAt, 180);
  assert.equal(deck.motion!.beatOnly, false);
  assert.equal(deck.motion!.direct, false);
});
