Initial OneLibraryCompanion distribution preview.

- Shared 1280 × 800 interface and Rust service, with standalone Mac and Raspberry Pi windows and unauthenticated LAN access.
- Mac supports close-to-background, Dock reopening, Quit, file exports and optional login startup. Pi packages include desktop and optional headless modes.

Targets: macOS 13+ (Apple Silicon/Intel); Raspberry Pi OS Bookworm 64-bit or later. Physical Intel/older macOS/Pi touchscreen and network testing remains required. Initial Mac packages are ad-hoc signed and not notarized. See docs/distribution.md for installation, data migration and known limitations.

Downloads:
- Apple Silicon Mac: `OneLibraryCompanion-0.1.0-macos-arm64.dmg`.
- Intel Mac: `OneLibraryCompanion-0.1.0-macos-x86_64.dmg`.
- Raspberry Pi desktop: download BOTH `onelibrarycompanion-host_0.1.0_arm64.deb` and `onelibrarycompanion_0.1.0_arm64.deb`, then install them together with `sudo apt install ./onelibrarycompanion-host_0.1.0_arm64.deb ./onelibrarycompanion_0.1.0_arm64.deb`.
- Mac ZIP alternatives, a source archive and `SHA256SUMS.txt` are also provided.

These release assets are built and smoke-tested by GitHub Actions from the tagged source. They are preview builds pending the hardware checks above.

Full source credits and dependency license texts are included in the source and application bundles. The repository and release downloads remain private.
