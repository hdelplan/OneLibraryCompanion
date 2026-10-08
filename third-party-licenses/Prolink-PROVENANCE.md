# Pinned Prolink sources

Source: https://github.com/usr-ein/prolink
Commit: `9d1b9c2a026ee0b55961e3b933845f3681b5ffc0`
License: GPL-3.0-only (see LICENSE).

The Rekordbox reader, protocol, networking and capture crates are included. The workspace is limited to these crates; upstream examples and external fixtures are omitted.

## Local modifications

- Expose the existing cue encoder through an offline diagnostic entry point, so native CDJ replies can be compared with OLC without publishing or sending a generated reply.

- Expose beat number, mounted USB/SD status, bar position and timing fields through `PlayerStatus`.
- Normalize cue playback and classify forward, held and reverse transport conservatively. Reverse classification requires ordinary play; loop direction can remain ambiguous. Effective synced tempo and local fader tempo are distinguished.
- Expose receive instants and packet counters. Shared ordering rejects duplicate or reordered packets, handles wrapping counters and permits recovery after a receive gap.
- Provide a bounded background packet recorder without adding protocol transmissions.
- Add a bounded GPL-3.0-only `exportExt.pdb` My Tags reader for definitions, assignments, category order and track joins. Validate page bounds and parent references while retaining incomplete optional assignments.
- Decode the 13-bit playlist slot count and respect presence masks for deleted rows.
- Expose a clone of the existing status socket for explicit, checked load requests. The monitor remains the sole receiver; shared-tap monitors provide no sender.

- Seed parsed analysis and artwork into served media while original audio stays in the disk-backed virtual filesystem.
- Serve full NFS audio reads up to 65404 bytes with a 128 KiB socket send buffer; preserve byte contents, file offsets and EOF. Bound READDIR separately.
- Encode separate hot/memory cue counts, point/loop flags and loop-end frame positions in dbserver replies, preserving exported cue identity and timing.

Cue metadata reference: https://djl-analysis.deepsymmetry.org/djl-analysis/track_metadata.html and Deep Symmetry Beat Link `CueList.parseNexusEntries`.

Protocol reference: https://djl-analysis.deepsymmetry.org/djl-analysis/vcdj.html (Deep Symmetry).

Review an explicit upstream commit when updating this subset, retain notices, and run application checks. Upstream tests requiring omitted fixtures are not part of the application suite.

## Serving behavior

- Keep original USB track IDs, compatible audio paths and metadata references in a single-library serving session.
- Read hot/memory categories independently from USB analysis, using extended tags per category and basic tags as fallback.
- Encode the legacy reply as memory entries in stored order followed by A/B/C in letter order, retaining original times, loop ends and co-located entries. Native CDJ-2000nexus replies provide binary regression expectations.
- Accept a caller-supplied portmapper socket for authorized desktop socket handoff; validate its bound address and own its lifetime.
- Supply available USB capacity/settings and filesystem audio timestamps, and keep a bounded serving event log without recording audio payloads.

The host supplies converted PCM only for eligible unsupported local audio; the protocol library serves the supplied files and exported analysis.
