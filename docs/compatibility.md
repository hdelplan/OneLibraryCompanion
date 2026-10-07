# Compatibility and operating limits

## Platforms

| Platform | Package baseline | Validation status |
| --- | --- | --- |
| Apple Silicon Mac | macOS 13+, ARM64 | Application startup, native window and host smoke checks exercised on a development Mac; minimum-OS coverage incomplete |
| Intel Mac | macOS 13+, x86-64 | Native CI build and host smoke checks; physical Intel/minimum-OS coverage incomplete |
| Raspberry Pi 4 / 5 | Raspberry Pi OS Bookworm 64-bit; Desktop for native UI, Lite for host-only use | ARM64 Bookworm build and host smoke checks; physical Pi runtime and touchscreen validation pending |
| LAN clients | A current Safari, Chrome, Firefox or Edge browser | Fixed 1280 × 800 layout scales to fit; browser/device coverage incomplete |

A successful build does not certify USB discovery, network operation, touch performance or sustained playback on every device. The release workflow runs application checks and host smoke tests before publishing installers; its logs are available in [GitHub Actions](https://github.com/hdelplan/OneLibraryCompanion/actions).

Mac packages are ad-hoc signed and not notarized. Windows, 32-bit Pi OS and operating systems older than these baselines do not have installers in this repository.

## Players and media

OLC displays two decks and exposes load controls for players numbered 1 and 2. The player model and firmware determine available status and audio-format support. Known format checks cover CDJ-2000nexus, CDJ-2000NXS2 and CDJ-3000; they are not a certification of all files or models.

Three-band waveforms require the corresponding exported analysis. Missing analysis is shown as unavailable. Beat-grid time and some loop positions are estimates, particularly on older players; precise sub-beat scratching is not guaranteed. Phrase sections and cues require those records in the export.

Linked CDJ libraries read legacy Rekordbox exports. Host-attached libraries read OneLibrary databases. Local USB loading uses Manual IP connections and virtual source number 4; see [local USB usage](local-usb.md). Source discovery on a first load may require LINK on the target CDJ. OLC does not force a start position after loading, and the player can land on a saved cue.

Loading requires fresh telemetry and a stopped target. Playing, looping, busy, stale and disconnected states block requests. A load confirmation reflects the player's reported selection, not proof of audible playback. There is no automatic retry or remote play command.

## Network, storage and history

Any device that can reach the OLC HTTP port has access to its controls and history. Local-network access has no accounts, password or pairing. Keep it on a trusted LAN; internet-facing deployment is not supported. Client isolation, a firewall or VPN routing may prevent connections.

The host must stay awake and running while serving local audio or recording a set. One host process owns the CDJ network session and data directory. USBs must be mounted and readable by that host user; OLC does not mount drives or write changes back to them.

Set history records reported CDJ playback, not mixer audibility. Tracks qualify after more than 45 seconds of continuous normal playback or active looping. Import requires dated, complete exported histories. Local OneLibrary history decoding is not available. PDF exports use rendered text and are printable, but their text is not selectable. Browser file sharing depends on browser support; downloading remains available.
