# Install and connect OLC

Download from [OLC Releases](https://github.com/hdelplan/OneLibraryCompanion/releases), signed in with a GitHub account that has access to this private repository.

## Mac

Requires **macOS 13 or later**.

1. Download **macos-arm64.dmg** for Apple Silicon or **macos-x86_64.dmg** for Intel.
2. Open the DMG and drag OneLibraryCompanion to Applications.
3. Open the app. If macOS blocks it, dismiss the warning and use **System Settings → Privacy & Security → Open Anyway** for the package you downloaded. OLC is not notarized by Apple.
4. For music on a USB attached to the Mac, choose **OneLibraryCompanion → Local USB Support… → Open Installer**. Approve installation, then quit and reopen OLC. Repeat this step after updating OLC.

Closing the window keeps OLC running. **Quit OLC** stops it. **Open at Login** is available in the application menu.

## Raspberry Pi Desktop

Use a **Pi 4 or 5 with Raspberry Pi OS Bookworm or later, 64-bit**. Download both ARM64 `.deb` packages from the same release into one folder. For version 0.2.2, run this command in that folder:

```sh
sudo apt install ./onelibrarycompanion-host_0.2.2_arm64.deb ./onelibrarycompanion_0.2.2_arm64.deb
```

Open OneLibraryCompanion from the applications menu. It has its own desktop window, with full-screen and login-startup options. A compatible touchscreen is supported; use the system's on-screen keyboard for typing. Mount the music USB through the operating system before selecting it in OLC.

## Network access

Connect the Mac/Pi and your CDJs to the same local network. In MENU:

- Use **Manual IP connections** for players 1 and 2 when loading from a USB attached to the computer. Subnet search helps find their addresses. Leave player number 4 free for OLC.
- For linked-CDJ operation, select the adapter connected to your players under **CDJ discovery interface**.

Save and restart OLC after changing connection modes. Select your library in BROWSE and load onto a stopped player.

To use a phone, tablet or another computer, open the network address shown in MENU, such as `http://192.168.1.20:8787`. No login or pairing is needed; everyone connected shares the controls and history. Keep this access on your local network.

Keep OLC running, the computer awake and the USB attached while serving music or recording a set.

## Raspberry Pi headless

To use a Pi only through other devices' browsers, install the host package on 64-bit Raspberry Pi OS Lite and start its user service:

```sh
sudo apt install ./onelibrarycompanion-host_0.2.2_arm64.deb
systemctl --user enable --now olc-host.service
```

Open the Pi's address on port 8787. Use either the desktop app or this service, not both at once. Booting without a user login requires the operating system's user-service lingering option. See the [development guide](development.md) for technical configuration.

## Startup problems

- **Players missing:** check the selected adapter, network connection, firewall and VPN routing.
- **Local USB missing or unable to load:** open MENU → Local USB and follow its message. On Mac, confirm Local USB Support was installed for this OLC version.
- **OLC cannot start:** follow the startup message. Another service may already be using its port.

[Local USB help](local-usb.md) · [Compatibility and known limits](compatibility.md).

## Data and backups

Quit OLC before copying the complete application data folder:

| Platform | Folder |
| --- | --- |
| Mac | `~/Library/Application Support/OneLibraryCompanion` |
| Pi | `~/.local/share/onelibrarycompanion`, or the configured XDG data location |

This preserves saved sets, artwork and host settings. Display preferences and filter presets are stored separately in each browser or app window. The Mac folder also contains `host.log` for startup troubleshooting.

## Remove Mac Local USB Support

Stop playback and quit OLC. In Finder, show the application's package contents and open **Contents → Resources → Remove Local USB Support.command**. Administrator approval is required. Music and saved sets are preserved.
