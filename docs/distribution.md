# OneLibraryCompanion distribution

OLC uses the same Rust backend and 1280 × 800 React interface on Mac and Raspberry Pi. Mac has a Swift/AppKit/WKWebView window; Pi uses GTK 3 and WebKitGTK 4.1. Neither desktop launcher opens the local browser. A single host serves the local window and every LAN browser, maintaining one CDJ session and one history recorder. Phone browsers currently show the scaled 1280 × 800 interface.

## Support targets

| Target | Initial minimum | Validation |
| --- | --- | --- |
| Apple Silicon and Intel Mac | macOS 13 | Deployment target; older OS and physical Intel validation required |
| Pi 4 / Pi 5, ARM64 desktop | Raspberry Pi OS Bookworm (Debian 12), 64-bit Desktop | Pi hardware validation pending; initial test device is Pi 4, 4 GB |
| Pi ARM64 headless | Raspberry Pi OS Bookworm Lite, 64-bit | Hardware validation pending |
| LAN browser | Modern Safari, Chrome, Firefox or Edge | Browser/device matrix validation required |

These are release targets, not a claim that every combination has passed hardware tests. Also validate Raspberry Pi OS Trixie. Use an OS-supported touchscreen; touch, multitouch, rotation, virtual keyboard and sustained waveform rendering require physical Pi testing.

## Mac installation

Download the installer matching Apple Silicon (`arm64`) or Intel (`x86_64`). Open the DMG and drag OneLibraryCompanion to Applications, or unzip the ZIP and move its app there. Launch OneLibraryCompanion. This initial distribution is ad-hoc signed, not notarized: macOS may require explicit approval in Privacy & Security. No paid Apple membership is required to build or use it.

Closing the window keeps OLC and LAN access running. Click its Dock icon or choose Show OLC to reopen. Quit OLC stops the owned host. Open at Login is optional in the application menu; registration may require moving the app to Applications and approving it in Login Items. Launching a second app instance brings forward the existing window.

A port conflict is reported instead of attaching to an unrelated service. Do not run the old development host and OLC on the same HTTP or CDJ UDP ports. The app does not stop existing services automatically.

Data lives under `~/Library/Application Support/OneLibraryCompanion`; `host.log` contains launcher service errors. Existing development history stays at the configured `PIONEER_COMPANION_DATA`/`OLC_DATA` directory or `.local/app-data`. To use it in the packaged app, quit both hosts and set `OLC_DATA` to that existing absolute directory before launching. No existing data is overwritten or moved automatically. Existing browser preferences persist at their current origin; native webviews have separate storage, and a different host IP or port creates a separate browser origin.

## Raspberry Pi installation

Use 64-bit Raspberry Pi OS. Install both downloaded packages on Desktop:

```sh
sudo apt install ./onelibrarycompanion-host_0.1.0_arm64.deb ./onelibrarycompanion_0.1.0_arm64.deb
```

Open OneLibraryCompanion from the applications menu or run `olc`. The OLC menu contains Full Screen, Open at Login, and Quit. Closing the window keeps the process alive; launching OLC again reopens it. Use the desktop's on-screen keyboard for search and metadata fields. USBs must be mounted by the OS and readable by the logged-in user; the host discovers mounted removable volumes, it does not mount disks or run as root.

For Lite/headless, install only the host package, then run:

```sh
systemctl --user enable --now olc-host.service
```

A user service normally needs an active user session; for boot without login, deliberately enable lingering for that user using the OS's `loginctl enable-linger` command. The package does not change this automatically. To stop:

```sh
systemctl --user disable --now olc-host.service
```

Desktop and headless modes are alternatives. Stop the headless service before starting the desktop app; they must not own competing Pro DJ Link listeners. Data lives under `$XDG_DATA_HOME/onelibrarycompanion` or `~/.local/share/onelibrarycompanion`.

## Network and connections

Desktop defaults to `0.0.0.0:8787`. MENU lists LAN URLs; open one in Safari or another browser on the same reachable network. There is no password, pairing or user account. Connected devices can browse, change shared history and request supported track loads. Do not forward this port to the internet. The host computer's firewall and Wi-Fi client isolation must allow the connection.

MENU offers a saved CDJ discovery interface; changes take effect after quitting and reopening the service. The default uses the existing manual-IP controls, including subnet search. Automatic discovery and manual-IP mode are alternatives. Environment overrides take precedence over saved settings. Remote browsers see USBs attached to the host and linked CDJs; a USB attached to a remote phone/tablet is not automatically exposed.

Downloads use native Save dialogs in desktop windows. Remote browser sharing uses the Web Share API when available, otherwise downloads. Sharing availability depends on the browser and its security context. Library browsing, histories and all supported production operations still use the same APIs. Existing player/format limits, approximate waveform positions and stopped-target load protections remain unchanged.

| Variable | Purpose |
| --- | --- |
| `OLC_UI_ROOT` | Absolute directory with bundled `index.html` and assets; set by launchers |
| `OLC_BIND` | HTTP bind address, default `0.0.0.0:8787`; launchers require a fixed nonzero port |
| `OLC_INTERFACE` | Override saved CDJ discovery interface |
| `OLC_DATA` | Existing/new writable history and desktop settings directory |
| `OLC_LIBRARY` | Optional legacy `export.pdb` source |
| `OLC_CAPTURE` | Optional saved analysis capture |
| `OLC_ROOT` | Development project root when not using bundled assets |

Existing `PIONEER_COMPANION_*` names remain aliases for these shared host settings. Browser display preferences remain per device; shared catalogue and history state live on the host. No `.env` file is loaded implicitly.

## Build and release

Install Rust (current lockfile/toolchain used by CI), Node 22.12+ and npm. Mac builds require Xcode command-line tools; Linux builds require Debian Bookworm's build-essential, pkg-config, perl, Python 3.11+ and dpkg-dev. SQLCipher/OpenSSL are built from the pinned dependencies; no private fixture is needed.

```sh
./scripts/setup.sh
./scripts/check-app.sh
./scripts/build-macos.sh arm64
# On Mac after: rustup target add x86_64-apple-darwin
./scripts/build-macos.sh x86_64
# On 64-bit Debian Bookworm; ARM64 for Pi packages:
./scripts/build-linux.sh
```

Mac build output is under `builds/macos-<arch>/OneLibraryCompanion.app`; release archives/installers and Debian packages are under `builds/releases`. Set `NODE` to a compatible Node executable if needed. `OLC_SKIP_DMG=1` builds only the Mac app and ZIP where disk-image creation is unavailable. Checksums accompany artifacts.

GitHub repository target: `hdelplan/OneLibraryCompanion`, private initially. `release.yml` builds both Mac architectures and ARM64 Debian packages, and attaches artifacts to a draft prerelease on version tags. Manual workflow runs produce Actions artifacts and can publish a prerelease when the publish option is selected. Linux builds use a Bookworm container so newer build-machine glibc does not silently raise the minimum. A local build on a newer Linux distribution must not be presented as Bookworm-compatible without checking its runtime dependencies.

Use `scripts/prepare-distribution.py` to produce a curated staging snapshot without captures, music, USB databases, credentials, build output. Review its manifest before pushing. Keep application source changes in this shared working tree and regenerate snapshots; do not maintain divergent copies. Retain GPL and third-party notices and the matching source for each release.

## Cross-building Pi packages on Mac

Install the ARM64 Rust target and the pinned cross tools in an isolated environment:

```sh
rustup target add aarch64-unknown-linux-gnu
python3 -m venv /tmp/olc-cross
/tmp/olc-cross/bin/pip install cargo-zigbuild==0.23.4 ziglang==0.15.2
PATH="/tmp/olc-cross/bin:$PATH" ./scripts/build-pi-cross.sh
```

The cross-build targets glibc 2.36 explicitly. Packaging verifies the ELF architecture and its highest GLIBC symbol version. On Mac, the packaging script writes the standard Debian ar/control/data container; on Debian it uses dpkg-deb. A successful cross-build is not proof of touchscreen, USB or Pro DJ Link behaviour on Pi hardware.

## Release validation gates

- Native startup, close/reopen, Quit, second-instance behaviour, file input/download, and LAN client sharing pass.
- Verify history/settings retention, library browse/filter/artwork, track-load protections, USB unplug/replug and reconnect.
- Test Mac Intel, macOS 13, Pi 4 touch/keyboard, and remote browsers before describing them as verified.

## Acknowledgments and license bundle

The source repository and every UI build include THIRD_PARTY_NOTICES.md, the full project license, source credits, and notices for pinned Cargo/npm dependencies. In a running web distribution they are available at `/licenses/THIRD_PARTY_NOTICES.md` and `/licenses/third-party-licenses/SOURCE-CREDITS.md`. Mac and Linux host packages additionally carry these notices in their Resources/documentation directories.

After dependency changes, install pinned dependencies, run `cargo metadata --locked` to populate the Cargo cache, then `python3 scripts/collect-third-party-notices.py`. The collector fails on missing notices instead of silently omitting dependencies. The manually retained source licenses and source credits must also be reviewed when source provenance changes.
