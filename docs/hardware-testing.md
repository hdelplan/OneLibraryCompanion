# Hardware verification

Run these checks with a supported Mac or Raspberry Pi host, representative exports and two CDJs on the same LAN. Record OS, player models/firmware, connection mode and results with the release being evaluated.

1. Install, launch, close/reopen and quit the native app. Check optional login startup and that only one host owns the session. Connect a LAN browser at the same time.
2. Connect players by manual IP and by automatic discovery. Compare playback state, BPM, pitch, master/sync, metadata and artwork with the physical players.
3. Check three-band waveforms, beat/cue markers, next-cue countdown, phrases and timing while playing, pausing, seeking and looping. Account for estimated position on older players.
4. Browse linked USBs and up to three host-attached OneLibrary drives. Check playlists, My Tags, filters, presets, sorting and touch row sizes. Unplug/reconnect idle drives and replace exports; stale selections must not be loaded.
5. In manual-IP mode, load original MP3, WAV and AIFF files from a local USB onto a stopped deck. Compare audible playback and speed with native USB playback. Check cold-start discovery, LINK prompts, cue positions and the visible load result. Load different tracks from the same USB to both decks and verify the first remains playable. Confirm a second local library requires stopping playback and restarting the connection session.
6. Verify loading is blocked for playing, looping, busy or stale targets and for formats without an available conversion path. For eligible unsupported files, check conversion progress, cache reuse, cancellation and WAV/AIFF output. Verify compatible audio remains unchanged and check hot A/B/C and memory cues, including coincident positions. An unknown result must not resend a command automatically.
7. Record, finish, edit, restore, reopen and export sets. Check the strict playback-duration threshold, interrupted-session recovery, imports and past-set browsing against available/missing tracks.
8. Disconnect/reconnect players and network links. Confirm stale state clears and that recovery does not attach old assets to a new track.
9. On Pi, check USB permissions, full screen, touchscreen gestures, on-screen keyboard, display rotation and sustained rendering/serving performance. Exercise Desktop and headless modes separately.

Current coverage is described in [compatibility](compatibility.md). Build success alone is not a hardware pass.

