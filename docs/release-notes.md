# OneLibraryCompanion 0.2.2

## Browse refresh fixes

- Preserve list position and playlist/filter selections through same-library refreshes after loading.
- Keep the existing rows visible during background refreshes, and disable loading until fresh library data arrives.

## Browse and navigation

- A larger **CDJ STATUS / BROWSE** toggle replaces the two separate buttons, using the same font size as the other navigation buttons. The active screen is highlighted; from MENU or SET HISTORY it returns to the last-used primary screen.
- BROWSE preserves list scroll position when returning from another screen. Changing collection, filters or sorting starts the new results at the top.
- Track titles currently playing on either connected deck appear in bright green immediately. Previously played titles retain softer green after qualifying playback.
- Screenshots use real exported waveform data for CDJ STATUS.

## UI refinements

- Load diagnostics have moved out of the normal MENU.
- Local USB controls are embedded in the MENU panel; BROWSE retains its USB selector.
- Conversion-capable CDJ buttons stay white. CDJ STATUS shows the converted format and bit depth on one line.
- Audio target selection remains; conversion benchmark controls have been removed from MENU.
- Played-track titles turn green without set recording and persist until **Clear played tracks** is used in the SET HISTORY banner.

## More music for older CDJs

**Built-in audio transcoding lets older players such as the CDJ-2000nexus play supported local files they cannot decode directly, including FLAC and ALAC.** Attach your OneLibrary USB to the Mac or Raspberry Pi, choose a track marked **TRANSCODE NEEDED**, and load it onto a stopped player. OLC prepares a compatible WAV or AIFF file before sending the load request.

- Automatic quality preservation or selectable 16-/24-bit WAV and AIFF output.
- Sample-rate conversion to 44.1 or 48 kHz when needed.
- Conversion progress, cancellation, prepared-file caching.
- Original USB files remain untouched; compatible tracks are sent as-is.
- Exported artwork, waveforms, beat grids, hot cues and memory cues accompany the track.
- Conversion runs inside OLC without additional audio software. It applies to host-attached USBs; linked CDJ USBs use their original files.

## One library, two players

Serve one local OneLibrary USB per connection session to physical players 1 and 2, with original track IDs and compatible audio paths. Different tracks remain available to both decks. OLC checks the destination is connected, fresh and stopped before loading. Use Manual IP connections and leave player number 4 free for OLC.

OLC also provides two-deck live status, three-band waveforms, playlist browsing, combined metadata filters, harmonic key highlighting, and reusable set history with text, CSV and PDF exports. Native windows and LAN browsers share the same host.

## Downloads and setup

- Apple Silicon: `OneLibraryCompanion-0.2.2-macos-arm64.dmg` or `.zip`.
- Intel Mac: `OneLibraryCompanion-0.2.2-macos-x86_64.dmg` or `.zip`.
- Raspberry Pi Desktop: install both `onelibrarycompanion-host_0.2.2_arm64.deb` and `onelibrarycompanion_0.2.2_arm64.deb`.
- Pi headless: install the host package and enable its user service.

Mac requires macOS 13+; Pi requires 64-bit Raspberry Pi OS Bookworm or later. Mac packages are ad-hoc signed and not notarized. On Mac, install **Local USB Support** from the application menu, then quit and reopen OLC. Reinstall this component after updating OLC. The Pi package grants the host its required port-binding capability.

Keep the host awake, OLC running and the USB attached throughout playback. Playback and cue controls remain on the physical CDJs. One local library can be served at a time; stop both players and reconnect OLC before switching libraries.

Matching source, license notices and SHA256 checksums accompany the installers. See the [installation guide](https://github.com/hdelplan/OneLibraryCompanion/blob/main/docs/distribution.md) and [compatibility](https://github.com/hdelplan/OneLibraryCompanion/blob/main/docs/compatibility.md) for platform coverage and operating limits.
