# Hardware verification

Use two CDJs and a supported Mac or Raspberry Pi host on the same LAN. Record
OS, player models/firmware, connection mode and results for the release tested.
Build success does not establish hardware coverage; see [compatibility](compatibility.md).

## Startup and connection

- Install, launch, close/reopen and quit the desktop app. Check full screen,
  optional login startup and a simultaneous LAN browser.
- Test automatic discovery and Manual IP connections. Confirm that only one host
  owns the session. Compare player numbers, metadata, BPM, pitch and master/sync
  with the physical players. Check unplug/reconnect and stale-state handling.
- On Pi, exercise Desktop and headless operation separately, including boot
  without login, mounted USB availability, touch gestures and text entry.

## Browse and playback

- Browse linked USBs and up to three host-attached OneLibrary drives. Check
  playlists, My Tags, search, filter presets, sorting and row sizes. Replace an
  idle export and verify stale selections cannot load.
- Load original MP3, WAV and AIFF tracks onto stopped decks using both connection
  modes. Check cold-start LINK discovery, audible playback, seeking, loops and
  hot/memory cues, including coincident positions. Load different tracks on both
  decks and verify the first remains playable.
- Verify playing, looping, busy, disconnected and stale targets reject loads.
  An uncertain result must not resend the command. Stop both decks before
  switching the local library or restarting the serving session.
- Test eligible unsupported audio, conversion progress, cancellation, cache reuse
  and WAV/AIFF outputs. Compare playback with the original; compatible files
  should remain unchanged.
- Check waveform detail, overview, beats, phrases and cue countdowns during play,
  pause, seek and loops. Account for paused non-master tracking limits.
- Preview local and linked tracks through the browser. Check play/pause, seeking,
  closing the popup and changing source; previews must not enter set history.

## Profiles, history and diagnostics

- Import saved My Settings, edit/duplicate profiles and transfer their JSON.
  Push to a stopped supported player and visually confirm settings. Verify
  playing/stale players reject commands and automatic rules apply once per
  connection with no automatic retry of skipped attempts.
- Capture, split, finish, join, edit, restore and export sets. Check duration/idle
  thresholds, restart recovery, imports and missing tracks in past-set playlists.
- In an isolated test, cause a load failure and verify a private incident report
  is saved with recent player states and serving evidence. Check bounded report
  retention and the optional one-minute browser report separately.
- On headless Pi, test guarded restart, shutdown and USB unmount with players
  disconnected. A busy volume must not be forcibly unmounted.

Finish with a sustained two-deck session while browsing, previewing and using a
remote screen. Keep failures and raw captures outside public source snapshots.
