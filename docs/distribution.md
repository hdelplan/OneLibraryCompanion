# Installation and operation

Download packages from [OLC Releases](https://github.com/hdelplan/OneLibraryCompanion/releases) while signed in to a GitHub account with access to the private repository. Choose the architecture of the computer running OLC. [Compatibility](compatibility.md) lists OS baselines and validation limits.

## Mac

Use the **macos-arm64** DMG for Apple Silicon or **macos-x86_64** for Intel. Open it and drag OneLibraryCompanion to Applications. A ZIP containing the same app is also available.

Launch OneLibraryCompanion from Applications. These packages are ad-hoc signed and not notarized. If macOS blocks the app, dismiss the warning, open **System Settings → Privacy & Security**, and approve the app with **Open Anyway** if you trust the downloaded package.

Closing the window keeps the service and LAN access running. Click its Dock icon or choose **Show OLC** to reopen it. **Quit OLC** stops the service. Launching the application a second time brings forward its window. **Open at Login** is available in the application menu; keep the app in Applications and approve the Login Item if macOS requests it.

## Raspberry Pi Desktop

Use 64-bit Raspberry Pi OS Bookworm or later. Download both ARM64 packages into the same directory, then install them together:

```sh
sudo apt install ./onelibrarycompanion-host_0.2.0_arm64.deb ./onelibrarycompanion_0.2.0_arm64.deb
```

Open OneLibraryCompanion from the applications menu or run `olc`. The native window uses GTK and WebKit; it does not open the local browser. Its menu includes **Full Screen**, **Open at Login** and **Quit**. Closing the window leaves OLC running; opening OLC again restores it.

Use an OS-supported touchscreen and the desktop's on-screen keyboard for text fields. Mount USBs through the operating system so they are readable by your user. OLC does not mount block devices or run as root. The host package uses `libcap2-bin` to grant its executable `CAP_NET_BIND_SERVICE`, permitting the UDP port 111 required by CDJs. Installation fails explicitly if the filesystem cannot retain this capability.

## Raspberry Pi headless

On 64-bit Raspberry Pi OS Lite, install only the host package, then enable the user service:

```sh
sudo apt install ./onelibrarycompanion-host_0.2.0_arm64.deb
systemctl --user enable --now olc-host.service
```

Open the Pi's address at port 8787 from a browser on the LAN. A user service normally needs a logged-in session. For boot without login, configure lingering for that user with `loginctl enable-linger` according to the OS setup. To stop the service:

```sh
systemctl --user disable --now olc-host.service
```

Use either desktop or headless mode on a host. Stop the headless service before launching the desktop application so only one service owns the CDJ connection.

## Network access

OLC listens on `0.0.0.0:8787` by default. MENU lists LAN URLs such as `http://192.168.1.20:8787`. Open the appropriate address from any device on the same reachable local network. There is no password or pairing. Every connected client can use the libraries, request permitted loads and edit shared history.

The host and CDJs need working local-network connectivity; firewalls, VPN routing and Wi-Fi client isolation can prevent it. Keep the service on a trusted LAN and do not forward its port to the internet. The host must remain awake while recording a set or serving music.

For local USB loading, use **Manual IP connections** and physical players 1 and 2. Automatic discovery on a selected adapter is available for linked-CDJ operation. Connection-mode changes require saving and restarting OLC. See [connections and preferences](configuration.md) and [local USB usage](local-usb.md).

For Mac-hosted local audio, install **Local USB Support** from the OneLibraryCompanion application menu, approve the macOS Installer prompt, then quit and reopen OLC. Repeat this setup after an OLC update. It installs the networking service and the matching host signature under `/Library`, without changing your music or saved sets. See [local USB usage](local-usb.md) for removal and operating limits.

## Data and backups

| Host | Default application data directory |
| --- | --- |
| Mac | `~/Library/Application Support/OneLibraryCompanion` |
| Raspberry Pi | `$XDG_DATA_HOME/onelibrarycompanion`, or `~/.local/share/onelibrarycompanion` |

History, copied artwork and saved host settings live here. On Mac, `host.log` contains service startup errors. Display preferences and filter presets are stored separately in each browser/native webview.

Quit OLC before backing up or moving the complete data directory. To use another persistent directory, set `OLC_DATA` to its absolute path in the host's launch environment. Existing directories are not migrated automatically. Only one host process may own the same data directory.

## Startup problems

If OLC cannot start, read the actual error in the dialog and host log. A port conflict means another listener owns the required address/port; identify that service before stopping it or configuring another fixed port with `OLC_BIND`. A stale status, missing library or player load failure is a separate connection/media problem; follow the message in the relevant screen.

For a failed Pi user service, inspect `systemctl --user status olc-host.service` and `journalctl --user -u olc-host.service`. Build configuration and environment variables are documented in the [development guide](development.md).

## Package contents and source

Mac installers include the native launcher, Rust host, web interface and license bundle. Pi separates the host/UI package from the desktop launcher package so the host can run without a desktop. Release downloads include a matching source archive and `SHA256SUMS.txt`.

The repository and downloads are private. Grant a person repository access through GitHub before sharing release links. Project licensing and third-party notices apply to source and binaries; see [acknowledgments](../THIRD_PARTY_NOTICES.md).
