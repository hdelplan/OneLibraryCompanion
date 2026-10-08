# OneLibraryCompanion 0.1.3

OLC provides two-deck live status with three-band waveforms, library browsing and filtering, protected track loading, host-attached OneLibrary USB playback, and persistent set history with text, CSV and PDF exports. Mac and Raspberry Pi desktop windows and LAN browsers share the same host.

## Installers

- Apple Silicon: `OneLibraryCompanion-0.1.3-macos-arm64.dmg`.
- Intel Mac: `OneLibraryCompanion-0.1.3-macos-x86_64.dmg`.
- Raspberry Pi Desktop: install both `onelibrarycompanion-host_0.1.3_arm64.deb` and `onelibrarycompanion_0.1.3_arm64.deb`.
- Pi headless: install the host package and enable its user service.

Mac ZIP alternatives, matching source and `SHA256SUMS.txt` accompany the installers. Mac requires macOS 13+; Pi requires 64-bit Raspberry Pi OS Bookworm or later. Mac packages are ad-hoc signed and not notarized. Physical Pi, Intel and minimum-OS validation remains incomplete.

## Local USB operation

Local browsing is available on Mac and Pi. On Mac, install Local USB Support from the application menu with administrator approval, then quit and reopen OLC. Its system service supplies UDP 111 only to the matching signed host executable. Reinstall the component after updating OLC. The Pi installer grants the host executable the port-binding capability needed for UDP 111.

Use Manual IP connections to physical players 1 and 2, leaving player number 4 free for OLC. Up to three mounted OneLibrary USBs are available. OLC serves original audio and exported metadata/analysis, keeps independent tracks available to both decks, and returns complete audio reads within the network payload limit.

Keep the host awake, OLC running and the USB connected during playback. First-load discovery may require LINK on the target player. Check the displayed load result and physical cue position; OLC does not force a start position or retry an uncertain load automatically.

See the README for features and `docs/distribution.md`, `docs/local-usb.md` and `docs/compatibility.md` for usage and limits. License notices and source credits are included in source and application packages.
