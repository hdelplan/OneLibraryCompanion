# OLC distribution validation — 2026-10-07

This is a distribution preview, not a certification across all supported hardware.

## Completed

- Shared application checks: 146 Rust tests and 89 UI tests pass; Rust formatting, strict Clippy, TypeScript including unused checks, Prettier and production Vite build pass.
- Apple Silicon and Intel Mac `.app`, ZIP and DMG builds succeed, use macOS 13.0 deployment targets, and pass ad-hoc signature verification. No notarization was performed.
- ARM64 Linux service cross-build succeeds with the glibc 2.36 target. ELF architecture is AArch64 and its highest required GLIBC symbol version is 2.34, within Bookworm's 2.36 baseline. Desktop and host Debian package containers, metadata, ownership, launcher symlink and dependencies are inspected.
- Native Mac window visually renders the shared 1280 × 800 interface; MENU contains LAN addresses and offline preview. Native file selection and saving a test history export work. Closing/reopening the window preserves the service; a separate browser sees the same saved history.
- Isolated host smoke test checks shared history persistence across immediate restart, network-setting persistence, duplicate service rejection and graceful shutdown with an SSE browser connected. Tests use temporary data and no CDJ connection/control.


- GitHub Actions successfully built and smoke-tested Apple Silicon, Intel and ARM64 Linux release assets for v0.1.0.
- Dependency licenses and source credits are bundled with the applications.

## Still required

- Physical Pi 4 / 4 GB installation and runtime testing: GTK/WebKit, touch and multitouch, on-screen keyboard, screen rotation/fullscreen, USB mounts and permissions, Pro DJ Link connectivity, waveform performance and long-running operation.
- Physical Intel Mac and macOS 13 validation; compilation alone does not certify runtime compatibility.
- Login-startup behaviour after installing to the final system location, plus installation/Gatekeeper behaviour on another Mac.
- The private GitHub repository https://github.com/hdelplan/OneLibraryCompanion has been created. The full source snapshot, acknowledgments and workflows are uploaded; the remote Git tree was verified to match the local snapshot. Hosted checks and distribution jobs are tracked under GitHub Actions.

The pre-existing development host and its data were not stopped or migrated. Native verification used a separate port and temporary history directory. No new CDJ load or playback commands were sent for distribution testing.
