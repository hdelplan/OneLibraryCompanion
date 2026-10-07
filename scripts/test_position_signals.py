import copy
import contextlib
import io
import os
import socket
import tempfile
import threading
import time
import types
import unittest
from pathlib import Path
from unittest.mock import patch

import position_signals as signals


def packet(kind, size, name=b"CDJ-2000nexus"):
    raw = bytearray(size)
    raw[:10] = signals.MAGIC
    raw[10] = kind
    raw[11:11 + len(name)] = name
    raw[31] = 1
    raw[33] = 2
    raw[34:36] = (size - 36).to_bytes(2, "big")
    return raw


def status(phase=1000):
    raw = packet(0x0A, 284)
    raw[0x89] = 0xF4
    raw[0x7B] = 8
    raw[0x9D] = 1
    raw[0x113] = 23
    raw[0xA0:0xA4] = (3).to_bytes(4, "big")
    raw[0xA6] = 3
    raw[0x92:0x94] = (12000).to_bytes(2, "big")
    raw[0x116:0x118] = (2000).to_bytes(2, "big")
    raw[0x11A:0x11C] = phase.to_bytes(2, "big")
    for offset in signals.SPEED_OFFSETS:
        raw[offset:offset + 4] = (1048576).to_bytes(4, "big")
    return raw


class DecoderTests(unittest.TestCase):
    def test_paused_coverage_distinguishes_master_playing_stopped_and_unknown(self):
        master, follower = status(), status()
        master[0x21] = 1
        master[0x7b] = 3
        master[0x89] = 0xf4
        follower[0x21] = 2
        follower[0x89] = 0x94
        follower[0x7b] = 5
        rows = []
        def add(at, raw, ip):
            rows.append({"kind": "udp", "port": 50002, "hostMs": at,
                         "ip": ip, "hex": raw.hex()})
        add(0, follower, "192.0.2.2")
        add(10, master, "192.0.2.1")
        add(20, follower, "192.0.2.2")
        follower[0x7b] = 8
        add(30, follower, "192.0.2.2")
        master[0x89] &= ~0x40
        master[0x7b] = 6
        add(40, master, "192.0.2.1")
        add(50, follower, "192.0.2.2")
        add(1500, follower, "192.0.2.2")
        coverage = signals.paused_nonmaster_coverage({"samples": rows})
        self.assertEqual([(g["mode"], g["masterTransport"]) for g in coverage],
                         [("paused", "unknown"), ("paused", "playing"),
                          ("cue scratch", "playing"), ("cue scratch", "stopped"),
                          ("cue scratch", "unknown")])
        self.assertTrue(all(g["finePhaseFrozen"] for g in coverage))

    def test_paused_coverage_missing_fine_fields_are_not_frozen_and_cue_audition_plays(self):
        master, follower = status(), status()[:0xd4]
        master[0x89] = 0xb4
        master[0x7b] = 7  # Cue audition can advance with playing flag clear.
        follower[0x89] = 0x94
        rows = [{"kind": "udp", "hostMs": at, "ip": ip, "hex": raw.hex()}
                for at, ip, raw in [(0, "192.0.2.1", master), (20, "192.0.2.2", follower)]]
        group, = signals.paused_nonmaster_coverage({"host": {"samples": rows}})
        self.assertEqual(group["masterTransport"], "playing")
        self.assertFalse(group["finePhaseObserved"])
        self.assertFalse(group["finePhaseFrozen"])

    def test_speed_comparison_uses_motion_and_rejects_gaps_and_track_changes(self):
        a = signals.decode(50002, status())
        a["states"] = [3, 250, 9, 5]
        a["speeds"]["0x98"] = 1.1
        beat = {"beatInBar": 1, "speed": 1.1, "nextBeatNominalMs": 500}
        next_beat = {**beat, "beatInBar": 2}
        stream = [(0, beat), (500 / 1.1, next_beat)]
        rows = [(0, a), (150, a), (300, a)]
        result = signals.speed_interval_comparison(rows, stream)
        self.assertEqual(result["usedIntervals"], 1)
        self.assertAlmostEqual(result["fields"]["0x98"]["cumulativeError"], 0)
        self.assertAlmostEqual(result["fields"]["0x8c"]["cumulativeError"], -500 / 11)
        self.assertEqual(signals.speed_interval_comparison(rows[:1], stream)["usedIntervals"], 0)
        changed = copy.deepcopy(a)
        changed["track"] += 1
        self.assertEqual(signals.speed_interval_comparison(rows[:2] + [(300, changed)], stream)["usedIntervals"], 0)
        self.assertEqual(signals.speed_interval_comparison(rows, [(0, beat), (950, {**beat, "beatInBar": 3})])["usedIntervals"], 0)

    def test_beat_comparison_preserves_real_phase_changes_and_missing_beats(self):
        compared = signals.beat_arrival_comparison([0, 500, 1000, 1500], [2, 550, 1498])
        self.assertEqual(compared["matched"], 3)
        self.assertEqual(compared["unmatchedLeft"], 1)
        self.assertEqual(compared["unmatchedRight"], 0)
        self.assertEqual([p["rightMinusLeftMs"] for p in compared["pairs"]], [2, 50, -2])
        self.assertEqual(signals.beat_arrival_comparison([], [1])["matched"], 0)

    def test_every_truncation_and_wrong_magic_are_safe(self):
        for raw, port in [(status(), 50002), (packet(0x28, 96), 50001),
                          (packet(0x0B, 60), 50001)]:
            for length in range(len(raw)):
                decoded = signals.decode(port, raw[:length])
                self.assertNotEqual(decoded["type"], "absolute-candidate")
            raw[0] = 0
            self.assertEqual(signals.decode(port, raw)["type"], "invalid")

    def test_speed_fields_and_sync_modes_stay_distinct(self):
        raw = status()
        raw[0x89] = 0xD6  # non-master, sync, tempo-only
        raw[0x98:0x9C] = (524288).to_bytes(4, "big")
        decoded = signals.decode(50002, raw)
        self.assertFalse(decoded["master"])
        self.assertTrue(decoded["tempoOnlySync"])
        self.assertEqual(decoded["speeds"]["0x8c"], 1)
        self.assertEqual(decoded["speeds"]["0x98"], .5)

    def test_nonmaster_beat_and_pitch_scaled_interval(self):
        raw = packet(0x28, 96)
        raw[95] = 2
        raw[36:40] = (500).to_bytes(4, "big")
        raw[84:88] = (2097152).to_bytes(4, "big")
        decoded = signals.decode(50001, raw)
        self.assertEqual(decoded["type"], "beat")
        self.assertEqual(decoded["nextBeatScaledMs"], 250)
        raw[95] = 1
        self.assertEqual(signals.decode(50001, raw)["type"], "beat-unattributed")

    def test_negative_position_never_uses_counter_as_player(self):
        raw = packet(0x0B, 60, b"CDJ-2000NXS2")
        raw[33] = 242
        raw[40:44] = (-12345).to_bytes(4, "big", signed=True)
        decoded = signals.decode(50001, raw)
        self.assertEqual(decoded["candidatePositionMs"], 12345)
        self.assertNotIn("player", decoded)
        raw[40:44] = bytes(4)
        self.assertNotEqual(signals.decode(50001, raw)["type"], "absolute-candidate")

    def test_3000_format_validates_duration_model_and_port(self):
        raw = packet(0x0B, 60, b"CDJ-3000")
        raw[36:40] = (300).to_bytes(4, "big")
        raw[40:44] = (12345).to_bytes(4, "big")
        self.assertEqual(signals.decode(50001, raw)["candidatePositionMs"], 12345)
        self.assertNotEqual(signals.decode(50004, raw)["type"], "absolute-candidate")
        raw[36:40] = (0x27FFFF50).to_bytes(4, "big")
        self.assertNotEqual(signals.decode(50001, raw)["type"], "absolute-candidate")

    def test_stagehand_telemetry_is_not_a_playhead(self):
        self.assertEqual(signals.decode(50004, packet(0x20, 60))["type"], "0x20")

    def test_repeated_phase_does_not_inflate_speed(self):
        a = signals.decode(50002, status(1100))
        b = signals.decode(50002, status(1228))
        pairs = signals.movement_pairs([(0, a), (64, a), (128, b)])
        self.assertEqual(len(pairs), 1)
        self.assertAlmostEqual(pairs[0]["phaseDerivedSpeed"], 1)
        self.assertAlmostEqual(pairs[0]["speedErrors"]["0x98"], 0)

    def test_direction_flags_are_ambiguous_and_track_changes_rejected(self):
        a = signals.decode(50002, status(1100))
        b = signals.decode(50002, status(1228))
        pairs = signals.movement_pairs([(0, a), (128, b), (256, a)])
        self.assertEqual([p["direction"] for p in pairs], ["forward", "backward"])
        self.assertEqual([p["state3"] for p in pairs], [1, 1])
        changed = copy.deepcopy(b)
        changed["track"] = 99
        self.assertEqual(signals.movement_pairs([(0, a), (128, changed)]), [])
        b["master"] = False
        self.assertEqual(signals.movement_pairs([(0, a), (128, b)]), [])

    def test_old_reports_and_unknown_packets_survive_analysis(self):
        report = {"host": {"samples": [{"kind": "udp", "hostMs": 1,
                   "ip": "10.0.0.2", "hex": status().hex()}]}}
        self.assertEqual(signals.analyze(report)["peers"]["10.0.0.2"]["finePairs"], 1)
        report["host"]["samples"].append({"kind": "udp", "hostMs": 2, "port": 50004,
                                          "ip": "10.0.0.2", "hex": packet(0x20, 60).hex()})
        self.assertIn("50004:0x20", signals.analyze(report)["peers"]["10.0.0.2"]["packetTypes"])


class ProbeTests(unittest.TestCase):
    def test_profiles_are_only_54_byte_announcements_with_real_address(self):
        mac = bytes.fromhex("020102030405")
        for profile, identity in [("observer", 15), ("bridge-c0", 192)]:
            raw = signals.keepalive(profile, "10.0.0.10", mac)
            self.assertEqual(len(raw), 54)
            self.assertEqual(raw[10], 6)
            self.assertEqual(raw[36], identity)
            self.assertEqual(raw[38:44], mac)
            self.assertEqual(raw[44:48], bytes([10, 0, 0, 10]))
            self.assertFalse(signals.collision(raw, "10.0.0.10", profile))
            self.assertTrue(signals.collision(raw, "10.0.0.11", profile))

    def test_experimental_profile_requires_explicit_selection(self):
        args = types.SimpleNamespace(local="10.0.0.10", peer=["10.0.0.2"], seconds=45,
                                     profile="bridge-c0", experimental_bridge=False)
        with self.assertRaisesRegex(ValueError, "experimental-bridge"):
            signals.listen(args)

    def test_existing_capture_is_never_overwritten_or_network_opened(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "existing.json"
            path.write_text("keep")
            args = types.SimpleNamespace(local="10.0.0.10", peer=["10.0.0.2"], seconds=45,
                                         profile="passive", mac=None, broadcast=None, output=str(path))
            with patch.object(signals.socket, "socket") as sock:
                with self.assertRaises(FileExistsError):
                    signals.listen(args)
                sock.assert_not_called()
            self.assertEqual(path.read_text(), "keep")

    @unittest.skipUnless(os.environ.get("PC_SOCKET_TESTS") == "1", "requires local socket access")
    def test_passive_capture_keeps_unknown_raw_packets_filters_peers_and_closes(self):
        # Use an ephemeral port; never compete with a real CDJ/application port.
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        errors = []
        def send():
            try:
                time.sleep(.3)
                with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sender:
                    sender.bind(("127.0.0.1", 0))
                    for _ in range(3):
                        sender.sendto(b"unknown-position-message", ("127.0.0.1", port))
                        time.sleep(.05)
            except Exception as error:
                errors.append(error)
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "capture.json"
            args = types.SimpleNamespace(local="127.0.0.1", peer=["127.0.0.2"], seconds=5,
                                         profile="passive", mac=None, broadcast=None, output=str(path))
            # Only the selected peer's unknown raw payload should survive.
            for expected in (0, 3):
                args.output = str(path.with_name(f"capture-{expected}.json"))
                args.peer = ["127.0.0.1" if expected else "127.0.0.2"]
                thread = threading.Thread(target=send)
                thread.start()
                with patch.object(signals, "PORTS", (port,)), contextlib.redirect_stdout(io.StringIO()):
                    signals.listen(args)
                thread.join()
                report = signals.json.loads(Path(args.output).read_text())
                self.assertNotIn("error", report)
                self.assertEqual(len(report["samples"]), expected)
                self.assertTrue(all(bytes.fromhex(s["hex"]) == b"unknown-position-message"
                                    and s["ip"] == "127.0.0.1" for s in report["samples"]))
                self.assertFalse(errors)
            # The receiver is closed after the bounded run.
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
                probe.bind(("127.0.0.1", port))


if __name__ == "__main__":
    unittest.main()
