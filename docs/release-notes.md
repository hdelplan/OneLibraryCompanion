Initial OneLibraryCompanion distribution preview.

- Shared 1280 × 800 interface and Rust service, with standalone Mac and Raspberry Pi windows and unauthenticated LAN access.
- Production desktop builds exclude TEST and experimental controls; the standalone iPad keeps TEST and receives OLC branding.
- Mac supports close-to-background, Dock reopening, Quit, file exports and optional login startup. Pi packages include desktop and optional headless modes.
- Existing internal storage identifiers and iPad installation identity are preserved.

Targets: macOS 13+ (Apple Silicon/Intel); Raspberry Pi OS Bookworm 64-bit or later. Physical Intel/older macOS/Pi touchscreen and network testing remains required. Initial Mac packages are ad-hoc signed and not notarized. See docs/distribution.md for installation, data migration and known limitations.
