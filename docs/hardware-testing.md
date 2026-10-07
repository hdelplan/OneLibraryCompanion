# Hardware validation checklist

Status: first single-player discovery, USB retrieval and play/pause observation completed; see [validation notes](research/live-validation-2026-09-29.md). Remaining scenarios below need hardware testing. Use a powered-on CDJ on the same network.

1. Confirm the existing Prolink discovery/status workflow still works; record player model, firmware and test date locally.
2. Verify USB metadata and analysis retrieval for a loaded track; compare title, artist, key and original BPM with the player.
3. Check play/pause/stop, actual BPM, pitch percentage/range, master and sync state. Unknown fields must remain unknown.
4. Change tracks and USB sources; ensure waveform and metadata switch together without stale data.
5. Seek and use cues on the physical player; verify position, remaining time, waveform alignment and beatgrid.
6. Disconnect/reconnect the network and restart the player; confirm stale/disconnected states and recovery.
7. Compare several bass-heavy tracks and quieter passages with the RX3 reference, including balanced/emphasis and centered/one-third playheads.
8. Measure smoothness and resource use on the intended target device.

The UI must not send loading, cue or playback commands. Keep diagnostic captures local. Mixer FX extraction and virtual-deck serving require separate investigation and are outside this checklist.

## Pending validation: smooth scrolling and phrases (2026-09-30)

Automated clock tests cover 60 Hz interpolation across 5 Hz snapshots, pitch, pause, seek, track replacement and update loss. Synthetic native PSSI tests cover masked/plain parsing, one-based beat mapping, duration bounds and missing grids. The CDJ was not detected during this iteration, so these changes still need a physical check:

1. Load a track with Rekordbox phrase analysis on the USB; confirm section labels/boundaries beneath the overview against Rekordbox or the device.
2. Play at normal and adjusted pitch; judge scrolling smoothness, then pause, cue, seek and loop across a boundary.
3. Disconnect the player and confirm motion stops and stale information clears.
4. Confirm both deck overviews fit and phrase visibility can be toggled in Configuration.

## Follow-up (2026-09-30): event delivery and native rendering

The subsequent iteration read a live track and its phrases successfully from the CDJ-2000nexus USB. Phrase colors and the 168px overview are visually verified. Canvas drawing was measured with four animated canvases. Jog/scratch accuracy has not been physically verified; the player exposes beat-based position, which cannot describe every sub-beat movement. See [the investigation and benchmark](research/latency-and-waveforms-2026-09-30.md).
