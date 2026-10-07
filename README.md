# OneLibraryCompanion (OLC)

OneLibraryCompanion connects your music libraries and Pioneer / AlphaTheta CDJs in one workspace. Browse and filter tracks, load a stopped player, follow both decks with three-band waveforms, and record a reusable set history. Playback, cueing and mixing stay on the physical players.

OLC runs as a standalone application on Mac or Raspberry Pi. Other devices on the same network can open its web interface and use the same connected players, USB libraries and saved sets. No account or pairing is required. The interface uses a fixed 1280 × 800 layout that scales to fit the display and supports mouse, keyboard and touch input.

## Download and install

[Download installers](https://github.com/hdelplan/OneLibraryCompanion/releases) · [Installation guide](docs/distribution.md)

| Platform | Package | Minimum OS |
| --- | --- | --- |
| Apple Silicon Mac | `macos-arm64.dmg` or `.zip` | macOS 13 |
| Intel Mac | `macos-x86_64.dmg` or `.zip` | macOS 13 |
| Raspberry Pi 4 / 5 | Both ARM64 `.deb` packages for the desktop app; host package only for headless use | Raspberry Pi OS Bookworm (Debian 12), 64-bit |

Mac packages are ad-hoc signed and not notarized. Physical Pi, Intel Mac and minimum-OS validation is incomplete; see [compatibility and operating limits](docs/compatibility.md). Repository access is required to download these private releases.

## Start using OLC

1. Put the OLC host and your CDJs on the same network. In **MENU**, use **Manual IP connections** to connect players 1 and 2; subnet search helps locate their addresses. Automatic discovery on a selected network interface is also available.
2. Choose a library in **BROWSE**. Use a CDJ-mounted USB, or attach a OneLibrary USB to the Mac/Pi and wait for it to appear. Local USB loading requires Manual IP connections.
3. Find a track, stop the destination player and press **CDJ1** or **CDJ2** beside the track. For a host-attached USB, keep OLC running and the drive connected throughout playback.
4. Use **CDJ STATUS** to follow playback. In **SET HISTORY**, start a set to record qualifying tracks, then **Finish & save** when finished.
5. For web access, open a LAN address shown in **MENU** on another device. The host must remain running.

## Features

### Follow a mix

- **Two-deck live status:** see each player's track, source, key, playback state, BPM, pitch, master and sync status together. Missing or stale telemetry is identified rather than presented as current playback.
- **Three-band waveforms:** scrolling detail separates low, mid and high frequency content. Whole-track overviews help locate sections. OLC reads exported analysis; it does not re-analyse the audio.
- **Beat, cue and loop guidance:** beat/bar markers, saved cues, available loop regions and a bars-and-beats countdown to the next hot cue help judge transitions. Phrase sections show the track's structure when the export includes phrase analysis.
- **Timing and sync feedback:** switch between elapsed and remaining time, zoom the waveform with buttons or a two-finger pinch, and see sync phase warnings when reliable beat data is available. Positions on older players remain estimates.
- **Track information:** open the information button for artwork, album, genre, mood, rating, color, My Tags, comments and audio format details supplied by the library.

### Find and load music

- **USB libraries and playlists:** browse nested playlist folders from linked CDJ exports, and up to three OneLibrary USBs attached to the host. Mounted local drives are checked every five seconds; library contents are read-only.
- **Search and sorting:** search metadata and sort the matching track list by its column headings. Three row sizes let you choose between more tracks and larger touch targets.
- **Combined filters:** narrow tracks by rating, BPM, color, musical key, genre, artist, label, format, date, year, duration and exported My Tags. My Tag selections support ANY, ALL or NONE matching. Active filter chips show what is restricting the results.
- **Personal filter layout and presets:** choose visible filters, their order and BPM menu bounds in MENU. Save named filter combinations for a particular USB export and recall them in BROWSE.
- **Mixing context:** key highlighting helps find harmonically related tracks; tracks recorded in the active set are marked as played. Known player-format incompatibilities are flagged before loading.
- **Protected track loading:** request a track on CDJ1 or CDJ2 without leaving the library. OLC checks the source, connection and stopped-player state before sending one load request. Playing, looping, busy or stale targets are blocked; uncertain requests are never retried automatically.
- **Host-attached USB playback:** OLC serves the selected original audio file and its exported metadata, artwork and analysis to a directly connected CDJ, without transcoding. Different tracks from different local libraries can remain available to both players. See [local USB usage](docs/local-usb.md) for setup and limits.

### Keep and reuse set lists

- **Set recording:** start a set explicitly; a track qualifies after more than 45 seconds of uninterrupted normal playback or active looping. Recording continues on the host while another screen is open or a web client disconnects.
- **Set editing:** name the set, add a location and comment, reorder or remove entries, and restore the original order. Saved metadata stays available after the USB is removed.
- **Recovery and management:** resume or finish an interrupted set after restarting OLC. Cancel an active set or delete a saved set with confirmation.
- **History import:** import complete Rekordbox histories with an unambiguous date in their names. Duplicate imports are ignored; undated or incomplete histories are skipped.
- **Browse past sets as playlists:** select SET HISTORY inside BROWSE to find tracks from a previous performance. OLC matches them against the selected USB; unavailable or ambiguous matches remain visible but cannot be loaded.
- **Export and sharing:** save the selected, last finished or all past sets as text, CSV or PDF. Share through a supported browser or prepare an email draft. Set history belongs to OLC and does not modify the USB's history.

### Use OLC your way

- **Display preferences:** adjust waveform zoom, playhead position/color, jog smoothing, band emphasis, time display, overviews, phrase analysis and additional details. Preferences are saved separately on each client.
- **Offline analysis preview:** open an exported analysis file in MENU to inspect its waveform without a connected CDJ. Preview scrubbing changes the display only.
- **Native and web access:** Mac and Pi desktop windows share one host with LAN browsers. Closing the window keeps the host running; Quit stops it. Optional login startup and a Pi headless user service are available.

## Guides

- [How to use each screen, with screenshots](docs/screenshots.md)
- [Connections, display and filter settings](docs/configuration.md)
- [Local USB libraries and loading](docs/local-usb.md)
- [Set history](docs/set-history.md)
- [Compatibility and operating limits](docs/compatibility.md)
- [Build and development guide](docs/development.md) · [Architecture](docs/architecture.md)

![Browse music in OLC](docs/screenshots/browse.png)

## License and acknowledgments

OLC is GPL-3.0-only. See [third-party acknowledgments](THIRD_PARTY_NOTICES.md), [source credits](third-party-licenses/SOURCE-CREDITS.md) and the [dependency inventory](third-party-licenses/dependency-inventory.json). Source and application packages include the applicable license texts. OLC is independent of Pioneer DJ, AlphaTheta and rekordbox.
