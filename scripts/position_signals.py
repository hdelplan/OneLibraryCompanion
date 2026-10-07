#!/usr/bin/env python3
"""Inspect saved jog reports, or run a bounded Pro DJ Link telemetry experiment.

No third-party dependencies. Decoded absolute positions are candidates, never
fed to the production clock. Protocol references and test procedure live in
docs/research/position-signals-audit.md.
"""
import argparse
import collections
import ipaddress
import json
import math
import pathlib
import selectors
import socket
import statistics
import time

MAGIC = b"Qspt1WmJOL"
PORTS = (50000, 50001, 50002, 50004)
SPEED_OFFSETS = (0x8C, 0x98, 0xC0, 0xC4)


def uint(data, offset, size=4):
    return int.from_bytes(data[offset:offset + size], "big")


def decode(port, data):
    if len(data) < 0x24 or data[:10] != MAGIC:
        return {"type": "invalid"}
    kind = data[10]
    result = {"type": f"0x{kind:02x}", "length": len(data)}
    if port == 50002 and kind == 0x0A and len(data) >= 0xD4:
        flags = data[0x89]
        result.update(type="status", player=data[0x21],
                      model=data[11:31].split(b"\0")[0].decode("ascii", "replace"),
                      firmware=data[0x7C:0x80].decode("ascii", "replace"),
                      track=uint(data, 0x2C), source=[data[0x28], data[0x29]],
                      master=bool(flags & 0x20), sync=bool(flags & 0x10),
                      tempoOnlySync=bool(flags & 2), playing=bool(flags & 0x40),
                      beat=uint(data, 0xA0), beatInBar=data[0xA6],
                      packetCounter=uint(data, 0xC8),
                      bpm=uint(data, 0x92, 2) / 100,
                      states=[data[0x7B], data[0x8B], data[0x9D],
                              data[0x113] if len(data) > 0x113 else None],
                      speeds={f"0x{o:02x}": uint(data, o) / 1048576
                              for o in SPEED_OFFSETS})
        if len(data) >= 0x11C:
            result["barSteps"] = uint(data, 0x116, 2)
            result["barPosition"] = uint(data, 0x11A, 2)
    elif port == 50001 and kind == 0x28 and len(data) == 96:
        if data[0x21] != data[0x5F] or not 1 <= data[0x21] <= 6:
            result["type"] = "beat-unattributed"
            return result
        speed = uint(data, 0x54) / 1048576
        nominal = uint(data, 0x24)
        result.update(type="beat", player=data[0x21], beatInBar=data[0x5C],
                      speed=speed, bpm=uint(data, 0x5A, 2) / 100,
                      nextBeatNominalMs=None if nominal == 0xFFFFFFFF else nominal,
                      nextBeatScaledMs=nominal / speed
                      if speed > 0 and nominal != 0xFFFFFFFF else None,
                      scratchWords=[uint(data, 0x58, 2), uint(data, 0x5D, 2)])
    elif port == 50001 and kind == 0x0B and len(data) == 60:
        signed = int.from_bytes(data[40:44], "big", signed=True)
        # Experimental NXS2 format: byte 33 is a counter, NOT a player number.
        # Keep source-IP attribution in the enclosing record. Zero is ambiguous.
        if signed < 0 and signed != -(2 ** 31):
            result.update(type="absolute-candidate", format="legacy-negative",
                          candidatePositionMs=-signed, counterByte=data[33])
        elif signed >= 0 and data[11:31].startswith(b"CDJ-3000") and 1 <= data[33] <= 6:
            length = uint(data, 36)
            if 0 < length < 86400 and signed <= length * 1000 + 1000:
                result.update(type="absolute-candidate", format="3000",
                              player=data[33], candidatePositionMs=signed,
                              trackLengthSeconds=length)
        # No inference from 0x20 on port 50004: Stagehand uses the same number
        # for telemetry that is NOT a playhead. Preserve its raw bytes.
    return result


def distribution(values):
    values = sorted(v for v in values if math.isfinite(v))
    if not values:
        return {"count": 0}
    return {"count": len(values), "min": values[0], "median": statistics.median(values),
            "p95": values[min(len(values) - 1, math.ceil(len(values) * .95) - 1)],
            "max": values[-1]}


def records(report):
    root = report.get("host", report)
    for sample in root.get("samples", []):
        if sample.get("kind") != "udp" or sample.get("direction", "rx") != "rx":
            continue
        try:
            raw = bytes.fromhex(sample["hex"])
        except (ValueError, KeyError):
            continue
        # Old reports only tapped the status receiver.
        port = sample.get("port", 50002)
        yield sample, raw, decode(port, raw)


def phase_beats(status):
    steps, phase = status.get("barSteps", 0), status.get("barPosition", 0)
    beat, within = status.get("beat", 0), status.get("beatInBar", 0)
    if steps <= 0 or not 0 <= phase <= steps or not 1 <= within <= 4 or not 1 <= beat < 0xFFFFFFFF:
        return None
    sub = phase / steps * 4
    # Reject beat/phase disagreements instead of guessing a bar at wrap.
    if not within - 1 <= sub <= within:
        return None
    return beat - within + sub


def movement_pairs(rows):
    """Net fine-phase speed vs raw speed; NOT external accuracy ground truth."""
    pairs = []
    anchor = None
    for tb, b in rows:
        if anchor is None:
            anchor = (tb, b)
            continue
        ta, a = anchor
        identity_a = (a["track"], a["source"], a["bpm"], a["states"][0], a["master"])
        identity_b = (b["track"], b["source"], b["bpm"], b["states"][0], b["master"])
        if identity_a != identity_b:
            anchor = (tb, b)
            continue
        if (a.get("barSteps"), a.get("barPosition"), a["beat"]) == (b.get("barSteps"), b.get("barPosition"), b["beat"]):
            continue
        # Timestamp the last phase change, not the last repeated status. Otherwise
        # a 128ms phase refresh amid 64ms status packets spuriously doubles speed.
        anchor = (tb, b)
        dt = (tb - ta) / 1000
        if not .025 <= dt <= .250 or not a["master"] or not b["master"]:
            continue
        pa, pb = phase_beats(a), phase_beats(b)
        if pa is None or pb is None or a["bpm"] <= 0:
            continue
        delta = (pb - pa) * 60 / a["bpm"]
        if abs(delta) < .020 or abs(delta / dt) > 4:
            continue
        measured = delta / dt
        pairs.append({"direction": "forward" if delta > 0 else "backward",
                      "phaseDerivedSpeed": measured, "state3": b["states"][2],
                      "state4": b["states"][3],
                      "speedErrors": {key: (a["speeds"][key] + b["speeds"][key]) / 2 - measured
                                      for key in b["speeds"]}})
    return pairs


def beat_arrival_comparison(left, right):
    """Pair nearest beat arrivals without forcing decks into phase.

    This compares receiver timestamps, not audio/sample alignment. Unmatched
    beats are reported explicitly; no offset is removed or smoothing applied.
    """
    intervals = [b - a for stream in (left, right) for a, b in zip(stream, stream[1:]) if b > a]
    if not intervals:
        return {"matched": 0, "unmatchedLeft": len(left), "unmatchedRight": len(right)}
    limit = statistics.median(intervals) / 2
    used = set()
    pairs = []
    for at in left:
        candidates = [(abs(other - at), index) for index, other in enumerate(right) if index not in used]
        if not candidates:
            break
        distance, index = min(candidates)
        if distance < limit:
            used.add(index)
            pairs.append({"leftMs": at, "rightMinusLeftMs": right[index] - at})
    return {"matched": len(pairs), "unmatchedLeft": len(left) - len(pairs),
            "unmatchedRight": len(right) - len(pairs),
            "offsetMs": distribution([p["rightMinusLeftMs"] for p in pairs]),
            "pairs": pairs}


def speed_interval_comparison(statuses, beats):
    """Compare held status speeds against independent beat-grid intervals.

    Errors are SOURCE milliseconds, not screen/network latency. Use only normal
    forward playback of the same track with status coverage no older than 250ms.
    Nominal interval is supplied by the earlier beat packet, not fitted to rates.
    """
    errors = {f"0x{offset:02x}": [] for offset in SPEED_OFFSETS}
    used = 0
    skipped = 0
    for (start, first), (end, second) in zip(beats, beats[1:]):
        nominal = first.get("nextBeatNominalMs")
        prior = [(t, d) for t, d in statuses if t <= start]
        # Do not interpret a lost beat or a seek as speed-estimation error.
        if (not prior or not nominal or nominal <= 0 or end <= start
                or second["beatInBar"] != first["beatInBar"] % 4 + 1
                or first["speed"] <= 0
                or not .5 < (end - start) / (nominal / first["speed"]) < 1.5):
            skipped += 1
            continue
        initial_at, initial = prior[-1]
        observations = [prior[-1]] + [(t, d) for t, d in statuses if start < t < end]
        intervals = list(zip(observations, observations[1:] + [(end, observations[-1][1])]))
        if start - initial_at > 250 or any(
            stop - at > 250 or d["track"] != initial["track"]
            or d["source"] != initial["source"] or not d["playing"]
            or d["states"][0] != 3 or d["states"][1] & 4 or d["states"][2] not in (9, 13)
            for (at, d), (stop, _) in intervals
        ):
            skipped += 1
            continue
        for key in errors:
            distance = sum((stop - max(start, at)) * d["speeds"][key]
                           for (at, d), (stop, _) in intervals)
            errors[key].append(distance - nominal)
        used += 1
    return {"usedIntervals": used, "skippedIntervals": skipped,
            "units": "source milliseconds; internal beat consistency, not physical accuracy",
            "fields": {key: {"signedError": distribution(values),
                              "meanAbsoluteError": statistics.mean(map(abs, values)) if values else None,
                              "cumulativeError": sum(values)} for key, values in errors.items()}}


def paused_nonmaster_coverage(report):
    """Separate test conditions; packet flags are evidence, not physical truth.

    In particular CUED/cue-scratch is not ordinary PLAY/PAUSE, and a master
    flag does not imply that the master is playing. Missing/stale master
    status must never be silently classified as stopped.
    """
    latest = {}
    groups = {}
    rows = sorted((row for row in records(report) if row[2]["type"] == "status"),
                  key=lambda row: row[0]["hostMs"])
    for sample, _, status in rows:
        at, ip = sample["hostMs"], sample["ip"]
        latest[ip] = (at, status)
        if status["master"] or status["playing"] or status["states"][0] == 7:
            continue
        masters = [s for peer, (t, s) in latest.items()
                   if peer != ip and 0 <= at - t <= 1000 and s["master"]]
        master_state = "unknown"
        if len(masters) == 1:
            master_state = "playing" if masters[0]["playing"] or masters[0]["states"][0] == 7 else "stopped"
        mode = status["states"][0]
        key = (ip, status["track"], tuple(status["source"]), mode,
               master_state, status["sync"], status["tempoOnlySync"])
        if key not in groups:
            groups[key] = {"ip": ip, "player": status["player"], "track": status["track"],
                           "source": status["source"], "primaryState": mode,
                           "mode": {3: "held playback", 5: "paused", 6: "cued",
                                    8: "cue scratch"}.get(mode, "other stopped state"),
                           "masterTransport": master_state, "sync": status["sync"],
                           "tempoOnlySync": status["tempoOnlySync"],
                           "firstMs": at, "lastMs": at, "statusCount": 0,
                           "finePairs": set(), "beats": set()}
        group = groups[key]
        group["lastMs"] = at
        group["statusCount"] += 1
        if "barSteps" in status and "barPosition" in status:
            group["finePairs"].add((status["barSteps"], status["barPosition"]))
        group["beats"].add(status["beat"])
    result = []
    for group in groups.values():
        pairs, beats = group.pop("finePairs"), group.pop("beats")
        result.append({**group, "distinctFinePairs": len(pairs),
                       "finePhaseObserved": bool(pairs),
                       "finePhaseFrozen": len(pairs) == 1,
                       "beatRange": [min(beats), max(beats)]})
    return result


def analyze(report):
    by_ip = collections.defaultdict(list)
    for sample, raw, decoded in records(report):
        by_ip[sample["ip"]].append((sample, raw, decoded))
    output = {"receivers": report.get("host", report).get("receivers"), "peers": {},
              "pausedNonMasterCoverage": paused_nonmaster_coverage(report),
              "limitations": ["Receive timestamps include unknown transport delay.",
                              "Fine-phase derivatives are internal consistency checks, not physical ground truth.",
                              "Missing packets cannot establish protocol/entitlement incompatibility."]}
    beat_streams = {}
    for ip, packets in by_ip.items():
        statuses = [(s["hostMs"], d) for s, _, d in packets if d["type"] == "status"]
        beats = [(s["hostMs"], d) for s, _, d in packets if d["type"] == "beat"]
        if beats:
            beat_streams[ip] = [t for t, _ in beats]
        absolute = [(s["hostMs"], d) for s, _, d in packets if d["type"] == "absolute-candidate"]
        entry = {"packetTypes": dict(collections.Counter(f'{s.get("port", 50002)}:{d["type"]}' for s, _, d in packets)),
                 "beatIntervalsMs": distribution([b[0] - a[0] for a, b in zip(beats, beats[1:])]),
                 "absoluteIntervalsMs": distribution([b[0] - a[0] for a, b in zip(absolute, absolute[1:])]),
                 "absoluteFormats": dict(collections.Counter(d["format"] for _, d in absolute))}
        if statuses:
            entry["model"] = statuses[0][1]["model"]
            entry["firmware"] = statuses[0][1]["firmware"]
            entry["roles"] = dict(collections.Counter(f'player{d["player"]}:master={d["master"]}:sync={d["sync"]}:tempoOnly={d["tempoOnlySync"]}' for _, d in statuses))
            entry["speedFields"] = {key: {**distribution([d["speeds"][key] for _, d in statuses]),
                                          "unique": len({d["speeds"][key] for _, d in statuses})}
                                     for key in statuses[0][1]["speeds"]}
            entry["finePairs"] = len({(d.get("barSteps"), d.get("barPosition")) for _, d in statuses})
            entry["speedVsBeatIntervals"] = speed_interval_comparison(statuses, beats)
            status_bytes = [raw for _, raw, d in packets if d["type"] == "status"]
            entry["changingByteOffsets"] = [f"0x{i:02x}" for i in range(min(map(len, status_bytes)))
                                            if len({raw[i] for raw in status_bytes}) > 1]
            pairs = movement_pairs(statuses)
            entry["phaseComparisons"] = {}
            for direction in ("forward", "backward"):
                subset = [p for p in pairs if p["direction"] == direction]
                entry["phaseComparisons"][direction] = {
                    "count": len(subset),
                    "state3": dict(collections.Counter(str(p["state3"]) for p in subset)),
                    "state4": dict(collections.Counter(str(p["state4"]) for p in subset)),
                    "absoluteSpeedError": {key: distribution([abs(p["speedErrors"][key]) for p in subset])
                                           for key in statuses[0][1]["speeds"]}}
        output["peers"][ip] = entry
    if len(beat_streams) == 2:
        left, right = sorted(beat_streams)
        output["beatArrivalComparison"] = {"leftIp": left, "rightIp": right,
                                           **beat_arrival_comparison(beat_streams[left], beat_streams[right])}
    return output


def keepalive(profile, local, mac):
    """Construct protocol fields independently; no transport/control commands."""
    if profile not in ("observer", "bridge-c0") or len(mac) != 6:
        raise ValueError("Unknown registration profile or invalid MAC")
    data = bytearray(54)
    data[:10] = MAGIC
    data[10] = 6
    name = b"PioneerCompanion" if profile == "observer" else b"TCS-SHOWKONTROL"
    data[12:12 + len(name)] = name
    data[32:36] = bytes([1, 2 if profile == "observer" else 1, 0, 54])
    data[36] = 15 if profile == "observer" else 0xC0
    data[37] = 1 if profile == "observer" else 0
    data[38:44] = mac
    data[44:48] = socket.inet_aton(local)
    data[48] = 2 if profile == "observer" else 3
    data[52] = 1 if profile == "observer" else 5
    data[53] = 0 if profile == "observer" else 0x20
    return bytes(data)


def collision(raw, local, profile):
    if len(raw) < 37 or raw[:10] != MAGIC:
        return False
    claimed = 15 if profile == "observer" else 0xC0
    if raw[10] in (4, 8):  # device-number claim or number-in-use response
        return raw[36] == claimed
    if raw[10] == 2 and len(raw) >= 47:
        return raw[46] == claimed
    if len(raw) < 54 or raw[10] != 6:
        return False
    if raw[44:48] == socket.inet_aton(local):
        return False
    return raw[36] == claimed or (profile == "bridge-c0" and raw[52] == 5)


def listen(args):
    local = str(ipaddress.IPv4Address(args.local))
    peers = {str(ipaddress.IPv4Address(p)) for p in args.peer}
    if local in peers and args.profile != "passive":
        raise ValueError("Local interface address cannot also be a CDJ address")
    if not 5 <= args.seconds <= 60:
        raise ValueError("Capture duration must be 5–60 seconds")
    if args.profile == "bridge-c0" and not args.experimental_bridge:
        raise ValueError("bridge-c0 requires --experimental-bridge; unverified on original nexus")
    if args.profile != "passive" and not args.mac:
        raise ValueError("Specify this interface's MAC with --mac for registration")
    mac = bytes.fromhex(args.mac.replace(":", "")) if args.mac else b""
    if args.profile != "passive" and (len(mac) != 6 or mac[0] & 1 or not any(mac)):
        raise ValueError("Invalid unicast MAC")
    destination = str(ipaddress.IPv4Address(args.broadcast)) if args.broadcast else None
    packet = keepalive(args.profile, local, mac) if args.profile != "passive" else None
    report = {"version": 1, "profile": args.profile, "local": local,
              "peers": sorted(peers), "broadcast": destination, "samples": [], "receivers": {},
              "notes": ["Experimental receiver-side capture, not a complete switch mirror capture.",
                        "Registration only; no load, play, sync, master or metadata requests.",
                        "Bridge C0/03 profile is unverified on CDJ-2000nexus. Silence is inconclusive."]}
    # Never overwrite an existing capture. Open before sending anything.
    with open(args.output, "x") as output, selectors.DefaultSelector() as selector:
        sockets = {}
        started = time.monotonic()
        next_send = started + 3  # Observe identity conflicts before registration.
        try:
            # The selected address must be the real source of outgoing traffic,
            # not just an address placed in the announcement payload.
            for target in sorted(peers | ({destination} if destination else set())):
                with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as route:
                    route.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
                    route.connect((target, 50000))
                    if route.getsockname()[0] != local:
                        raise ValueError(f"Route to {target} uses {route.getsockname()[0]}, not {local}")
            for port in PORTS:
                sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                sockets[port] = sock
                # Bind exclusively. Do not share a live app's observer sockets.
                sock.bind(("0.0.0.0", port))
                sock.setblocking(False)
                if destination:
                    sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
                selector.register(sock, selectors.EVENT_READ, port)
                report["receivers"][str(port)] = {"bound": True}
            print(f'Capturing {args.profile} for {args.seconds}s. Keep players on the same tracks.', flush=True)
            while time.monotonic() - started < args.seconds:
                now = time.monotonic()
                if packet and now >= next_send:
                    for target in [destination] if destination else sorted(peers):
                        sockets[50000].sendto(packet, (target, 50000))
                        report["samples"].append({"kind": "udp", "direction": "tx", "port": 50000,
                                                  "hostMs": (time.monotonic() - started) * 1000,
                                                  "ip": target, "hex": packet.hex()})
                    next_send = now + 1.5
                for key, _ in selector.select(.05):
                    raw, sender = key.fileobj.recvfrom(65535)
                    if key.data == 50000 and packet and collision(raw, local, args.profile):
                        raise RuntimeError("Observer/bridge identity conflict detected; stopped registration")
                    if sender[0] not in peers:
                        continue
                    report["samples"].append({"kind": "udp", "direction": "rx", "port": key.data,
                                              "hostMs": (time.monotonic() - started) * 1000,
                                              "ip": sender[0], "sourcePort": sender[1],
                                              "length": len(raw), "hex": raw[:4096].hex(),
                                              "truncated": len(raw) > 4096})
                if len(report["samples"]) >= 12000:
                    report["limitReached"] = True
                    break
        except (OSError, RuntimeError, ValueError, KeyboardInterrupt) as error:
            report["error"] = str(error) or "Interrupted"
        finally:
            for sock in sockets.values():
                sock.close()
            report["durationMs"] = (time.monotonic() - started) * 1000
            json.dump(report, output, indent=2)
    print(json.dumps(analyze(report), indent=2))
    if "error" in report:
        raise SystemExit(report["error"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    analysis = commands.add_parser("analyze")
    analysis.add_argument("report", type=pathlib.Path)
    capture = commands.add_parser("listen")
    capture.add_argument("--local", required=True)
    capture.add_argument("--peer", action="append", required=True)
    capture.add_argument("--output", required=True)
    capture.add_argument("--seconds", type=int, default=45)
    capture.add_argument("--profile", choices=["passive", "observer", "bridge-c0"], default="passive")
    capture.add_argument("--mac")
    capture.add_argument("--broadcast", help="Interface's subnet broadcast; omitted means unicast to peers")
    capture.add_argument("--experimental-bridge", action="store_true")
    args = parser.parse_args()
    if args.command == "analyze":
        print(json.dumps(analyze(json.loads(args.report.read_text())), indent=2))
    else:
        listen(args)


if __name__ == "__main__":
    main()
