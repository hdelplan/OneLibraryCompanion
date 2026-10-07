import { recordJogSnapshot, syncJogTrace } from "./jogTrace";
import { useEffect, useState } from "react";
import type { Analysis, Deck, LivePlayer } from "./model";
import type { DirectPeer } from "./ManualLibrary";
type Snapshot = {
  jogCapture?: { active: boolean; session: string; remainingMs?: number };
  enabled: boolean;
  error: string | null;
  decks: LivePlayer[];
  directPeers?: DirectPeer[];
};
export function playerSlots(players: LivePlayer[]): (LivePlayer | undefined)[] {
  const slots = [
    players.find((p) => p.number === 1),
    players.find((p) => p.number === 2),
  ];
  for (const player of [...players].sort((a, b) => a.number - b.number)) {
    if (player.number <= 2) continue;
    const index = slots.findIndex((p) => !p);
    if (index >= 0) slots[index] = player;
  }
  return slots;
}
const empty: Analysis = { detail: null, preview: null, track: null };
// Pin an observation to its first browser arrival; SSE heartbeats and backend
// timer snapshots must not renew the lifetime of an old UDP packet.
export class ObservationTimes {
  private samples = new Map<number, { key: string; id: string; at: number }>();
  resolve(player: LivePlayer, now: number): number {
    const age = Math.max(0, player.positionAgeMs ?? player.statusAgeMs ?? 0);
    const at = now - age;
    if (!player.observationId) return at;
    const previous = this.samples.get(player.number);
    const key = player.trackKey ?? "";
    const result =
      previous?.key === key && previous.id === player.observationId
        ? Math.min(previous.at, at)
        : at;
    this.samples.set(player.number, {
      key,
      id: player.observationId,
      at: result,
    });
    return result;
  }
}
export function liveDeck(
  player: LivePlayer,
  analysis: Analysis = empty,
  receivedAt = performance.now(),
  observedAt = receivedAt -
    Math.max(0, player.positionAgeMs ?? player.statusAgeMs ?? 0),
): Deck {
  const connected = player.connection === "connected";
  return {
    name: player.name,
    source: "live",
    analysis: connected ? analysis : empty,
    position: connected ? player.position : null,
    live: connected
      ? player
      : {
          ...player,
          playState: null,
          bpm: null,
          pitch: null,
          master: null,
          sync: null,
          position: null,
        },
    motion:
      connected && player.position !== null
        ? {
            key: player.trackKey ?? "",
            cued: player.playState === "cued",
            position: player.position,
            receivedAt,
            observedAt,
            observationId: player.observationId ?? undefined,
            quality: player.positionQuality,
            playing:
              player.playing ??
              ["playing", "looping", "cue play"].includes(
                player.playState ?? "",
              ),
            rate:
              player.positionSource === "beat-motion" &&
              player.motionRate != null
                ? player.motionRate
                : (player.reverse ? -1 : 1) * (1 + (player.pitch ?? 0) / 100),
            tempoRate: 1 + (player.pitch ?? 0) / 100,
            beatNumber: player.beatNumber ?? undefined,
            beatOnly: player.positionSource === "status-beat-estimate",
            loop:
              player.playState === "looping"
                ? (player.loop ?? undefined)
                : undefined,
            direct:
              player.manualMotion === true ||
              player.playing === false ||
              player.positionSource === "direct-status-beat" ||
              (player.positionSource === "bar-phase" &&
                player.manualMotion !== false) ||
              ["searching", "cue scratch"].includes(player.playState ?? "") ||
              !!player.reverse,
          }
        : undefined,
  };
}
export function useLiveDecks() {
  const [state, setState] = useState<{
    enabled: boolean;
    error: string | null;
    decks: [Deck, Deck];
    directPeers: DirectPeer[];
  }>({ enabled: false, error: null, decks: [null, null], directPeers: [] });
  useEffect(() => {
    let stopped = false,
      lastMessage = performance.now();
    const observationTimes = new ObservationTimes();
    const assets = new Map<string, Analysis>();
    const pending = new Set<string>();
    const stream = new EventSource("/api/live/events");
    function accept(snapshot: Snapshot) {
      syncJogTrace(snapshot.jogCapture);
      recordJogSnapshot(snapshot.decks);
      lastMessage = performance.now();
      const receivedAt = performance.now();
      const slots = playerSlots(snapshot.decks);
      const players = slots.filter((p): p is LivePlayer => !!p);
      // Status updates must not wait for the larger analysis transfer.
      const publish = () => {
        if (!stopped)
          setState({
            enabled: snapshot.enabled,
            error: snapshot.error,
            directPeers: snapshot.directPeers ?? [],
            decks: [0, 1].map((i) =>
              slots[i]
                ? liveDeck(
                    slots[i],
                    assets.get(slots[i].trackKey ?? ""),
                    receivedAt,
                    observationTimes.resolve(slots[i], receivedAt),
                  )
                : null,
            ) as [Deck, Deck],
          });
      };
      publish();
      for (const player of players) {
        if (
          !player.assetReady ||
          !player.trackKey ||
          assets.has(player.trackKey) ||
          pending.has(player.trackKey)
        )
          continue;
        const key = player.trackKey;
        pending.add(key);
        void fetch(`/api/live/analysis/${player.number}`, {
          signal: AbortSignal.timeout(5000),
        })
          .then(async (response) => {
            if (!response.ok) return;
            const result: { key: string; analysis: Analysis } =
              await response.json();
            if (!stopped && result.key === key)
              assets.set(key, result.analysis);
          })
          .catch(() => {})
          .finally(() => pending.delete(key));
      }
      // Apply downloaded data on the next fresh status snapshot, never an older one.
      if (assets.size > 12)
        for (const key of assets.keys())
          if (!players.some((p) => p.trackKey === key)) assets.delete(key);
    }
    function disconnected() {
      if (!stopped)
        setState((prev) => ({
          ...prev,
          error: "Connection to host lost",
          decks: prev.decks.map((deck) =>
            deck?.live
              ? liveDeck({
                  ...deck.live,
                  connection: "disconnected",
                  position: null,
                  bpm: null,
                  pitch: null,
                  master: null,
                  sync: null,
                  playState: null,
                })
              : null,
          ) as [Deck, Deck],
        }));
    }
    stream.onmessage = (event) => {
      try {
        accept(JSON.parse(event.data) as Snapshot);
      } catch {
        disconnected();
      }
    };
    stream.onerror = disconnected;
    const watchdog = setInterval(() => {
      if (performance.now() - lastMessage > 1500) disconnected();
    }, 500);
    return () => {
      stopped = true;
      stream.close();
      clearInterval(watchdog);
    };
  }, []);
  return state;
}
