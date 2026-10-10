# OneLibraryCompanion 0.3.0

Changes since the published Mac/Raspberry Pi 0.2.3 preview.

## Automatic discovery and local USB loading

- Choose **Automatic discovery** in MENU to find CDJs on the local network. OLC reports the selected interface and searches again after network changes or disconnection. Manual IP connections and explicit interface selection remain available.
- Local OneLibrary USB loading now works through automatic discovery for players 1 and 2. Leave player number 4 free for OLC; Mac serving still requires Local USB Support. One local library is served per connection session.
- Overlapping load requests to the same player are rejected. OLC displays the local USB label for its served tracks.

## Browser audio preview

- Open **PLAY** in track details or hold a track title to open the browser player. Audio plays on the browser device, independently of the physical CDJs.
- The popup includes play/pause, waveform seeking, artwork, time, BPM, key, genre, My Tags, comments and library color. Exported phrase sections and hot/memory cue markers appear when available.
- Audio streams from host-attached or linked CDJ libraries, subject to browser format support. Local AIFF/AIFC files receive a temporary 44.1 kHz/16-bit WAV copy; original USB files and CDJ serving are unchanged.
- Analysis previews now cover local and automatically discovered sources, combine DAT/EXT/2EX records, and retain valid cues and phrases when waveform data is missing.

## Automatic set capture

- Capture runs on the host without pressing Start set. More than 45 seconds of continuous playback or looping qualifies a track; browser preview is excluded.
- More than five minutes without playback across the observed decks saves the session. Long tracks keep the session open; empty sessions are discarded.
- **Split set here** separates performances, **Finish set** saves immediately, and **Join previous set** joins adjacent captured sessions while preserving edited track orders.
- Start timestamps and repeat plays are retained. Short pauses, observation outages and restart recovery are handled without unnecessarily duplicating entries; uncertain observation boundaries are noted.

See the [set history guide](set-history.md) for qualification, recovery and editing details.

## Headless Pi controls

- MENU offers confirmed **Restart OLC**, **Shut down host** and **Unmount USB** controls on the packaged Linux user service.
- Restart and shutdown block loaded OLC USB tracks unless fresh player status confirms they are stopped. Confirmation messages explain the emergency-loop and track-reload risk.
- USB unmounting briefly stops OLC, handles mounted partitions together, reports completion and never forcibly unmounts a busy volume. Devices containing system mounts are excluded.
- Shutdown and USB unmounting require the permissions described in the [installation guide](distribution.md#raspberry-pi-headless).

## Connection reliability

- Fixed a post-load disconnection caused by repeatedly copying cached waveform assets. Assets are cached once per track identity and serving packets drain without rebuilding the UI snapshot for every packet.
- Failed USB analysis reads retry automatically, and the host records the last disconnection reason and timestamp for diagnostics.

## Other performance and reliability improvements

- Lossless packed waveform transfers reduced sampled responses by approximately 93%, with older uncompressed clients still supported.
- Progress-aware downloads tolerate slow transfers, cancel superseded tracks, retry failures and display completed analysis promptly.
- Relative beat-phase refinement supports eligible normal forward non-master playback. Small timing corrections are smoothed; known loops use circular correction while preserving wraps. Relative refinement excludes loops, reverse and manual motion; physical accuracy remains to be fully validated.
- MENU can show recent waveform frame/drawing measurements and position-jump counts. iPad Safari supports launching OLC from a Home Screen icon without browser tabs or its address bar.
- Updated Pi packaging, network capabilities, dependencies, regression checks and setup documentation.

## Downloads and setup

- Apple Silicon: `OneLibraryCompanion-0.3.0-macos-arm64.dmg` or `.zip`.
- Intel Mac: `OneLibraryCompanion-0.3.0-macos-x86_64.dmg` or `.zip`.
- Raspberry Pi Desktop: install both `onelibrarycompanion-host_0.3.0_arm64.deb` and `onelibrarycompanion_0.3.0_arm64.deb`.
- Pi headless: install the host package, enable lingering and the user service, then open the Pi's network address from another device. Configure local USB mounting at boot.

Mac requires macOS 13+; Pi requires 64-bit Raspberry Pi OS Bookworm or later. Mac packages are ad-hoc signed and not notarized. Reinstall **Local USB Support** after updating the Mac app.

Stop players using OLC-served tracks before updating packages or restarting the host. Keep OLC running, the host awake and the USB attached during playback.

[Installation and headless setup](distribution.md) · [Local USB and transcoding](local-usb.md) · [Compatibility and limits](compatibility.md).
