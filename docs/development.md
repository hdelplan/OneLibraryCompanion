# Build and development guide

## Requirements

Use Rust 1.98.1 (the CI toolchain), Node 22.12+ and npm. The Rust dependencies require at least Rust 1.88; CI is the reference build environment. Mac builds need Xcode command-line tools. Linux builds need a C toolchain, pkg-config, Perl, Python 3.11+ and dpkg-dev. SQLCipher and OpenSSL are built from pinned dependencies.

```sh
./scripts/setup.sh
./scripts/check-app.sh
cargo run --locked -p pioneer-companion-host
```

Open `http://127.0.0.1:8787`. Set `OLC_BIND=127.0.0.1:8787` for local-only development; the default permits LAN access. Use a separate port and data directory for isolated checks. Only one service should own a physical CDJ network session.

`check-app.sh` checks application Rust formatting, tests, strict Clippy, compilation, UI formatting, TypeScript including unused declarations, UI tests and the production UI build. Tests use synthetic data rather than private music or exports. Socket-based checks need local TCP/UDP permissions.

Format application packages with `cargo fmt -p pioneer-companion-core -p pioneer-companion-host` and UI sources with `npm --prefix source/ui run format`. Avoid formatting the entire vendored tree. Preserve source provenance and third-party notices.

## Desktop packages

```sh
./scripts/build-macos.sh arm64
# For cross-building Intel on Mac, install x86_64-apple-darwin first.
./scripts/build-macos.sh x86_64
# On Debian Bookworm ARM64:
./scripts/build-linux.sh
```

Mac apps are written under `builds/macos-<arch>`; DMGs, ZIPs, Debian packages and checksums go under `builds/releases`. `NODE` can select a compatible Node executable. `OLC_SKIP_DMG=1` skips disk-image creation while retaining the app and ZIP.

The Pi package grants `CAP_NET_BIND_SERVICE` for CDJ portmapper access on UDP 111 and `CAP_NET_RAW` for binding Pro DJ Link sockets to the selected network interface. Its install hook and capability are checked by CI. Mac builds include a Local USB Support installer that pins the hardened host executable signature. `scripts/test-mac-networking.py` checks live-process authentication without installing a system service. After administrator-approved installation, run the packaged `olc-host --check-local-usb-helper` to verify port handoff as an ordinary user.

The GitHub distribution workflow builds both Mac architectures and ARM64 Debian packages, runs application checks and host smoke checks, and packages source and checksums. A version tag creates a draft prerelease. A manual run with **publish** selected publishes the completed prerelease. The chosen tag must match `source/host/Cargo.toml`; retain that version in the Mac bundle and release guide as well.

Linux release builds use a Debian Bookworm container. This establishes the glibc baseline; packages built on a newer Linux host must not be assumed Bookworm-compatible.

## Cross-building ARM64 on Mac

```sh
rustup target add aarch64-unknown-linux-gnu
python3 -m venv /tmp/olc-cross
/tmp/olc-cross/bin/pip install cargo-zigbuild==0.23.4 ziglang==0.15.2
PATH="/tmp/olc-cross/bin:$PATH" ./scripts/build-pi-cross.sh
```

The cross-build targets glibc 2.36. Packaging verifies ELF architecture and required GLIBC symbols. Hardware checks remain necessary for USBs, touch, network operation and sustained playback.

## Host configuration

| Variable | Purpose |
| --- | --- |
| `OLC_BIND` | HTTP listen address; default `0.0.0.0:8787`. Desktop launchers require a fixed nonzero port. |
| `OLC_INTERFACE` | CDJ discovery adapter; overrides the saved setting. Without a selected adapter, use Manual IP connections. |
| `OLC_HOST_CONTROLS` | `systemd-user` enables confirmed restart/shutdown controls on Linux; supplied by the packaged headless service. Shutdown requires passwordless permission for `/usr/bin/systemctl poweroff`. |
| `OLC_DATA` | Persistent writable directory for history, artwork and host settings. |
| `OLC_UI_ROOT` | Directory containing `index.html` and assets; desktop launchers supply the bundled location. |
| `OLC_ROOT` | Development project root used to locate UI assets. |
| `OLC_LIBRARY` | Optional legacy `export.pdb` for a browse-only local source and offline metadata matching. |
| `OLC_CAPTURE` | Optional saved ANLZ analysis file for offline preview. |

Use absolute paths. The corresponding `PIONEER_COMPANION_*` variables are compatibility aliases; an `OLC_*` value takes precedence. OLC does not load a `.env` file implicitly. Packaged data paths are described in [installation](distribution.md#data-and-backups); development defaults to `.local/app-data` under the project root.

## Source snapshots and documentation

`scripts/prepare-distribution.py <new-directory>` creates an allowlisted source snapshot and `SOURCE-MANIFEST.sha256`. It excludes private exports, captures, credentials, dependencies and generated build output. Publication documentation is explicitly selected, including the screenshot gallery. Platform source roots and build scripts are allowlisted. Optional `.local/distribution-overrides` files replace matching allowlisted paths during export; they cannot add files outside that selection. It refuses to overwrite an existing destination.

Keep shared changes in `source/ui`, `source/host` and the native launchers. Review the snapshot and manifest before publishing. Read the remote branch before updating it so edits made on GitHub are retained. Documentation describes current behaviour: feature explanations belong in README and user guides; implementation contracts belong in architecture. Document only implemented features, current behaviour and known limitations. Do not include future plans, proposed features, roadmaps, development diaries or completed-work checklists.

The root license, source credits and dependency notices must accompany source and binaries. After dependency changes, install the locked dependencies, populate the Cargo cache with `cargo metadata --locked`, then run `python3 scripts/collect-third-party-notices.py`. The collector fails when notices are missing. Review manually maintained source credits when adapting new code or protocol references.

## Verification

Use `python3 scripts/smoke-host.py <host-binary> --ui <ui-directory>` for isolated host persistence, restart, duplicate-listener and shutdown checks. Use the [hardware checklist](hardware-testing.md) for release qualification. CI logs supply build results; [compatibility](compatibility.md) records current platform limits without duplicating transient test counts.
