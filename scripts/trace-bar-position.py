#!/usr/bin/env python3
"""Read opt-in raw packet logs; emit diagnostic JSONL without controlling decks.

Usage: python3 scripts/trace-bar-position.py LOG [--follow] [--peer IP]
Bar phase is not an absolute timestamp and a nonzero value may be stale.
"""
import argparse
import json
import math
import time


def decode(line):
    try:
        stamp, port, peer, raw = line.split()
        data = bytes.fromhex(raw)
        if (port != "status" or len(data) < 0x11c
                or data[:10] != b"Qspt1WmJOL" or data[10] != 0x0a):
            return None
        number = lambda offset, size: int.from_bytes(data[offset:offset + size], "big")
        length, position = number(0x116, 2), number(0x11a, 2)
        return {
            "receivedUs": int(stamp), "peer": peer.rsplit(":", 1)[0],
            "player": data[0x21], "sourcePlayer": data[0x28],
            "sourceSlot": data[0x29], "trackId": number(0x2c, 4),
            "state": data[0x7b], "beat": number(0xa0, 4),
            "beatInBar": data[0xa6], "sequence": number(0xc8, 4),
            "barSteps": length, "barPosition": position,
            "phaseBeats": 4 * position / length if length and position < length else None,
            "pitch2Multiplier": number(0x98, 4) / 0x100000,
        }
    except (ValueError, IndexError):
        return None


class Trace:
    def __init__(self):
        self.previous = {}

    def observe(self, row):
        key = (row["peer"], row["player"])
        identity = (row["sourcePlayer"], row["sourceSlot"], row["trackId"])
        old = self.previous.get(key)
        if old and (old["identity"] != identity or row["receivedUs"] < old["time"]
                    or row["sequence"] < old["sequence"]):
            old = None
        pair = (row["barSteps"], row["barPosition"])
        changed = bool(old and pair != old["pair"])
        # First observation is not proof of a fresh update. An unchanged value
        # may mean either a stopped deck or stale phase during normal playback.
        change_time = row["receivedUs"] if changed else old["change"] if old else None
        row["phaseChanged"] = changed
        row["msSincePhaseChange"] = ((row["receivedUs"] - change_time) / 1000
                                      if change_time is not None else None)
        row["packetIntervalMs"] = (row["receivedUs"] - old["time"]) / 1000 if old else None
        self.previous[key] = {"identity": identity, "pair": pair, "change": change_time,
                              "time": row["receivedUs"], "sequence": row["sequence"]}
        return row


def map_position(row, asset):
    """Diagnostic candidate only; raw phase may be stale or cross a beat late."""
    row["candidateSeconds"] = None
    phase = row["phaseBeats"]
    if phase is None or not 1 <= row["beatInBar"] <= 4:
        return row
    row["phaseBeatAgrees"] = math.floor(phase) + 1 == row["beatInBar"]
    if not asset or asset.get("analysis", {}).get("track", {}).get("id") != row["trackId"]:
        return row
    grid = asset.get("beatGridSeconds", [])
    # Beat numbers are one-based; phase is zero-based within the bar.
    index = row["beat"] - row["beatInBar"] + phase
    lower = math.floor(index)
    if row["phaseBeatAgrees"] and 0 <= lower < len(grid) - 1:
        row["candidateSeconds"] = grid[lower] + (index - lower) * (grid[lower + 1] - grid[lower])
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log")
    parser.add_argument("--follow", action="store_true")
    parser.add_argument("--peer")
    parser.add_argument("--analysis", help="Saved /api/live/analysis/PLAYER JSON; use only with the matching source media")
    args = parser.parse_args()
    trace = Trace()
    asset = None
    if args.analysis:
        with open(args.analysis) as source:
            asset = json.load(source)
    with open(args.log) as source:
        while True:
            offset = source.tell()
            line = source.readline()
            if not line or not line.endswith("\n"):
                if not args.follow:
                    break
                source.seek(offset)
                time.sleep(0.1)
                continue
            row = decode(line)
            if row and (not args.peer or row["peer"] == args.peer):
                print(json.dumps(map_position(trace.observe(row), asset)), flush=True)


if __name__ == "__main__":
    try:
        main()
    except (KeyboardInterrupt, BrokenPipeError):
        pass
