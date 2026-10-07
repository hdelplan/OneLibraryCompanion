# OLC distribution validation — 2026-10-07

This is a distribution preview, not a certification across all supported hardware.

## Completed

- Shared application checks: 146 Rust tests and 89 UI tests pass; Rust formatting, strict Clippy, TypeScript including unused checks, Prettier and production Vite build pass.
- Experiment-enabled backend tests pass; paired UI asset checks prove production excludes the TEST chunk and diagnostic endpoints, while the iPad bundle includes them.
- Apple Silicon and Intel Mac `.app`, ZIP and DMG builds succeed, use macOS 13.0 deployment targets, and pass ad-hoc signature verification. No notarization was performed.
- Signed iPad development build 30 succeeds with display name OLC, full product name OneLibraryCompanion, original bundle identifier, and minimum iPadOS 17.6. This includes iPadOS 17.6.1 and keeps TEST.
- ARM64 Linux service cross-build succeeds with the glibc 2.36 target. ELF architecture is AArch64 and its highest required GLIBC symbol version is 2.34, within Bookworm's 2.36 baseline. Desktop and host Debian package containers, metadata, ownership, launcher symlink and dependencies are inspected.
- Native Mac window visually renders the shared 1280 × 800 interface; MENU contains LAN addresses and offline preview. Native file selection and saving a test history export work. Closing/reopening the window preserves the service; a separate browser sees the same saved history.
- Isolated host smoke test checks diagnostic GET/POST rejection, shared history persistence across immediate restart, network-setting persistence, duplicate service rejection and graceful shutdown with an SSE browser connected. Tests use temporary data and no CDJ connection/control.

- Attribution refresh: full notices for 105 Cargo packages and 48 installed npm packages (all 26 production npm packages), copied research-source licenses, original upstream notices and consolidated source credits. Mac installers, Pi Debian packages and signed iPad build were refreshed. The paired distribution check compares every bundled notice byte-for-byte with the source files.

## Still required

- Physical Pi 4 / 4 GB installation and runtime testing: GTK/WebKit, touch and multitouch, on-screen keyboard, screen rotation/fullscreen, USB mounts and permissions, Pro DJ Link connectivity, waveform performance and long-running operation.
- Physical Intel Mac and macOS 13 validation; compilation alone does not certify runtime compatibility.
- Run the new iPad build on the existing iPad, and check Safari LAN access from a separate iPad/iPhone.
- Login-startup behaviour after installing to the final system location, plus installation/Gatekeeper behaviour on another Mac.
- The private GitHub repository https://github.com/hdelplan/OneLibraryCompanion has been created. The full source snapshot, acknowledgments and workflows are uploaded; the remote Git tree was verified to match the local snapshot. Hosted checks and distribution jobs are tracked under GitHub Actions.

The pre-existing development host and its data were not stopped or migrated. Native verification used a separate port and temporary history directory. No new CDJ load or playback commands were sent for distribution testing.
