# Pinned Prolink reader

2026-10-05 fractional-loop correction: original CDJ-2000nexus build-19 test-2
packets show P3=1 during forward half/quarter loops. Reverse classification is
therefore restricted to ordinary play (primary 3); loop direction remains
ambiguous. `position_requires_direct_updates` centralizes conservative motion
handling for both live adapters. No registration or transmission changes.

Source: https://github.com/usr-ein/prolink
Commit: `9d1b9c2a026ee0b55961e3b933845f3681b5ffc0`
License: GPL-3.0-only (see LICENSE).

The Rekordbox reader, protocol, networking and capture crate sources are included.
The workspace member list is narrowed to these crates. Networking/protocol/capture examples and external integration-test fixtures are omitted; reader source is unchanged.
Local patch: `prolink/src/monitor.rs` exposes the already-decoded status beat number as `PlayerStatus.beat_number` for approximate display positioning. The monitor also exposes reverse playback from play-state byte 0x9d when primary play state is 3, following Beat Link protocol research. No packet encoding or serving behavior is changed.
Upstream integration tests reference upstream fixtures not shipped here and are not part of the app test suite.
The separate ignored third-party/prolink checkout retains local My Tags and protocol experiments.
To update, review an explicit upstream commit and replace this subset; run the app checks.

Additional local research patches expose optional bar-length/position fields at 0x116/0x11a and beat-in-bar 0xa6 through PlayerStatus, so changes emit monitor events. An opt-in PIONEER_COMPANION_PACKET_LOG recorder writes received beat/status datagrams using a bounded background queue. Neither patch adds protocol transmissions.

Cue-play normalization: primary state 0x07 implies advancing playback even when status flag 0x40 is clear (120 captured CDJ-2000nexus cue-play packets). Other states retain the flag, including paused loops. A monitor regression test covers cue playback, release, and paused loop states.

LIBRARY browser patches (2026-10-03): `PlayerStatus` now exposes mounted USB/SD presence from the existing status packet reader, including when no track is loaded. `prolink-rekordbox/src/mytags.rs` adds a bounded `exportExt.pdb` reader based on the ignored local `third-party/prolink/crates/prolink-rekordbox/src/mytags.rs` experiment (GPL-3.0-only): definition table 3, assignment table 4, category/name/order and track-ID joins. The adapted reader rejects invalid row/page bounds, incomplete referenced pages and tag definitions with missing category parents. Assignments to absent tag definitions are retained for caller diagnostics. Application integration tests use synthetic pages; no private database fixtures were copied. Live validation on 2026-10-03 read four categories and 40 values from a CDJ-2000nexus USB. Its extension also retains assignments to three absent definitions; the application preserves available tags and treats missing optional assignments quietly.

Explicit load integration: Monitor retains an Arc to its already-bound UDP 50002 socket and exposes it through `status_sender()`. The monitor remains the sole receiver; the application uses the clone only for explicit, checked, single-send load requests. Shared-tap monitors expose no sender. No automatic load or play behavior is added to the monitor.

Playlist row-count correction (2026-10-03): read the 13-bit slot count packed at page-header 0x18 instead of treating the low byte and unrelated 0x22 bookkeeping as alternative counts. A live USB page with 284 slots and bookkeeping value 43 previously lost later index groups, hiding all 95 entries of House for Rasa and truncating other playlists. Presence masks still exclude deleted rows. Synthetic host regression fixtures cover high count bits and deletion; no private export bytes are included.

### 2026-10-03 jog observation handling

`monitor.rs` now exposes the accepted UDP receive instant and sender packet counter in StatusObservation. Shared StatusPacketOrder rejects duplicate/reordered counters after observing advancement, supports wrapping counters, permits a reset after a one-second receive gap, and leaves fixed/missing-counter senders usable. The direct-IP observer uses this same helper. Twenty monitor unit tests passed; the separate broad vendor run was limited by missing exported-library fixtures.

2026-10-04 synced transport correction: Pitch2 (local fader) differing from Pitch1 (effective synced tempo) no longer implies jogging while Sync is enabled. The P2 held/stopped bit still detects a held platter during play/loop. Unsynced pitch-difference handling remains unchanged. Both direct-IP and desktop paths use this decoder. Protocol basis: https://djl-analysis.deepsymmetry.org/djl-analysis/vcdj.html (Pitch1–4 and P2). Synthetic application regression covers both deck numbers and master states; physical two-player validation remains necessary.

2026-10-04 build 14 motion tracking: `PlayerStatus` exposes unsigned 0x98 `motion_pitch` and a conservative `forward_transport` gate (primary state 3, playing flag, no P2 hold, P3 9/13). Same decoder feeds direct-IP and monitor paths. Captures 9 and 13 validate forward nudges against independent received beat intervals; this is not a signed scratch-position decoder. Application tests cover both master states, pause/hold/loop/reverse exclusions and exact rate decoding. No protocol transmissions are added.

2026-10-05 build 17: `forward_transport` includes primary loop state 4, still requiring playing, no held-platter P2 bit and P3 9/13. The shared application tracker additionally refuses loop prediction unless grid-aligned whole-bar endpoints are available. Reverse-loop and pause exclusion are decoder regressions; loop wrap/resize and master independence are application regressions. No network control messages added.

2026-10-05 build 18 transport classification: reverse now requires active primary 3/4, playing flag, P3=1 and no held-platter bit; this distinguishes pause/hold ambiguity and covers reverse loops. Forward cue audition (primary 7) may qualify independently of the playing flag, subject to no held bit and P3 forward state. Application tests cover these cases; no signed paused-scratch speed is inferred. Protocol cross-check: https://djl-analysis.deepsymmetry.org/djl-analysis/vcdj.html .
