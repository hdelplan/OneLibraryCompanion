# OneLibraryCompanion (OLC) — iPad

Current distribution: build 30 is branded OLC, retains TEST and targets iPadOS 17.6+, including 17.6.1. Keep the existing bundle identifier and Personal Team setup when upgrading the existing installation. Shared source remains in `source/ui` and `source/host`. The original project/scheme filenames remain PioneerCompanion for continuity. See [current distribution guide](../../docs/distribution.md) and [validation status](../../docs/build-validation.md).

The following historical preview notes describe earlier milestones; they are not the current feature or validation checklist.

## Historical preview notes

Open `PioneerCompanion.xcodeproj` and use the shared PioneerCompanion scheme.
Target: iPadOS 17.6+, iPad only, landscape. This is an offline preview under
development, not a device-validated full port.

The build compiles the existing React UI and an in-process Rust/Axum static
library. No Mac server is used at runtime. Open an ANLZ analysis file through
Configuration's existing file input to preview waveforms. File picker behavior
still needs validation on a real iPad. Full local export-folder browsing and
live status/beat networking are not wired into this target yet. Manual-IP USB library browsing, artwork and offline waveform previews are now available in LIBRARY; see [manual-IP testing](../../docs/ipad-direct-ip-test.md).

## Build and signing

1. Open Xcode and finish the license/first-run setup with iOS platform support.
2. Ensure Rust's `aarch64-apple-ios` target is installed (already done on this Mac).
3. Ensure the UI dependencies are installed. The build script selects a compatible
   Node runtime, falling back to this Mac's bundled Codex Node when necessary.
4. For device installation, select your signing team and replace the placeholder
   `local.pioneercompanion.preview` bundle ID with your unique identifier.
5. Connect/trust the iPad, enable Developer Mode, select it in Xcode and Run.

The target does not enable the multicast entitlement. A Personal Team can be used
for the offline preview with Apple's seven-day expiry. The files in Configuration
are future live-networking templates, not an entitlement grant.

For a simulator build, install `aarch64-apple-ios-sim` on an Apple Silicon Mac
(or `x86_64-apple-ios` on an Intel Mac) and a simulator runtime first. That path
has not been validated here.

## Lifecycle

The server binds only to `127.0.0.1:8787`, keeping the WebView origin stable for
saved preferences. It ignores desktop environment variables and starts no
Pro DJ Link session. A native banner identifies offline mode. Backgrounding
stops the service and discards transient preview state; reopening requires
selecting the analysis again. The screen stays awake while the preview is active.

## Verification status

Portable regression checks passed: 48 Rust tests, 33 UI tests, formatting,
Clippy, TypeScript, desktop build and production UI build. The new server test
covers HTTP UI/health, invalid analysis, disabled load commands and port reuse
after shutdown. Xcode project and plist syntax are valid.

Native compilation, iOS dependency compilation and linking now pass with Xcode 26.3 for arm64/iPadOS 17.6+. The unsigned app is in `builds/ipad/DerivedData/Build/Products/Debug-iphoneos/PioneerCompanion.app` from the repository root. The macOS Bash empty-array issue was fixed, and the landscape preview requires full screen. No signed IPA has been produced or installed; file selection, rendering, lifecycle and persistence still require physical-device testing.

See [setup and build command](../../docs/ipad-preparation.md) and the
[Apple request draft](../../docs/apple-networking-entitlement-request.md).
