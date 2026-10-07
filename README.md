# OneLibraryCompanion (OLC)

A companion for Pioneer / AlphaTheta CDJs: shared live status and waveforms, USB library browsing and supported track loading, local OneLibrary browsing, and set history with text/CSV/PDF exports. Playback controls remain on the physical players.

The Mac and Raspberry Pi distributions have standalone application windows and serve the same interface to devices on the local network. The fixed 1280 × 800 layout is retained. Desktop production builds exclude TEST and experimental features; the standalone iPad keeps TEST. Shared behaviour stays in `source/ui` and `source/host`.

## Install and supported platforms

See [distribution and installation](docs/distribution.md) for Mac DMG/ZIP, Raspberry Pi Debian packages, LAN connections, data locations and configuration.

Initial targets are macOS 13+ on Apple Silicon and Intel, and Raspberry Pi OS Bookworm 64-bit or later. Physical Pi/Intel/older-OS testing is still required. Existing iPadOS 17.6.1 compatibility is preserved. Mac downloads are initially ad-hoc signed and non-notarized; paid Apple membership is not required.

## Develop

Requirements: Rust 1.88 or newer (CI pins the validated toolchain), Node 20.19+ in the 20.x line or 22.12+, npm; C toolchain and Perl for bundled SQLCipher/OpenSSL.

```sh
./scripts/setup.sh
./scripts/check-app.sh
cargo run --locked -p pioneer-companion-host
```

Open http://127.0.0.1:8787. MENU contains network settings and offline preview. The desktop host listens on the local network by default; every reachable device can access it without pairing. Use `OLC_BIND=127.0.0.1:8787` for a local-only development session. Do not run competing Pro DJ Link services.

Use manual-IP connections in MENU, or select a CDJ-facing interface and restart. `OLC_INTERFACE` overrides that selection. `OLC_LIBRARY`, `OLC_CAPTURE`, and `OLC_DATA` configure optional export/capture/data paths; the older `PIONEER_COMPANION_*` settings remain aliases. No music, USB database or capture is needed for a clean build.

```sh
./scripts/build-macos.sh arm64
./scripts/build-macos.sh x86_64
# On 64-bit Debian Bookworm (ARM64 for Pi):
./scripts/build-linux.sh
```

For iPad, retain the existing Xcode project and bundle identity; [iPad setup](source/ipad/README.md). Its build script produces a separate `dist-ipad` interface so desktop production assets never replace iPad TEST assets.

## Documentation

- [Distribution, configuration and validation](docs/distribution.md)
- [Release notes](docs/release-notes.md)
- [Shared architecture](docs/architecture.md)
- [Display and library configuration](docs/configuration.md)
- [Set history](docs/set-history.md)
- [Third-party acknowledgments](THIRD_PARTY_NOTICES.md)
- [Source credits and research references](third-party-licenses/SOURCE-CREDITS.md)
- [Pinned dependency inventory](third-party-licenses/dependency-inventory.json)

Some older development/research documentation describes earlier versions; use current source and distribution documentation for packaging behaviour. The app remains GPL-3.0-only with pinned Prolink sources under `vendor/prolink`. Preserve third-party notices in source and binary distributions.
