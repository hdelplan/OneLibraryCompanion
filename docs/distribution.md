# Install and connect OLC

Download from [OLC Releases](https://github.com/hdelplan/OneLibraryCompanion/releases).

## Mac

Requires **macOS 13 or later**.

1. Download **macos-arm64.dmg** for Apple Silicon or **macos-x86_64.dmg** for Intel.
2. Open the DMG and drag OneLibraryCompanion to Applications.
3. Open the app. If macOS blocks it, dismiss the warning and use **System Settings → Privacy & Security → Open Anyway** for the package you downloaded. OLC is not notarized by Apple.
4. For music on a USB attached to the Mac, choose **OneLibraryCompanion → Local USB Support… → Open Installer**. Approve installation, then quit and reopen OLC. Repeat this step after updating OLC.

Closing the window keeps OLC running. **Quit OLC** stops it. **Open at Login** is available in the application menu.

## Raspberry Pi Desktop

Use a **Pi 4 or 5 with Raspberry Pi OS Bookworm or later, 64-bit**. Download both ARM64 `.deb` packages from the same release into one folder. For version 0.3.0, run this command in that folder:

```sh
sudo apt install ./onelibrarycompanion-host_0.3.0_arm64.deb ./onelibrarycompanion_0.3.0_arm64.deb
```

Open OneLibraryCompanion from the applications menu. It has its own desktop window, with full-screen and login-startup options. A compatible touchscreen is supported; use the system's on-screen keyboard for typing. Mount the music USB through the operating system before selecting it in OLC.

## Network access

Connect the Mac/Pi and your CDJs to the same local network. In MENU:

- Automatic discovery supports local USB loading to players 1 and 2. **Manual IP connections** and subnet search remain available. Leave player number 4 free for OLC.
- For linked-CDJ operation, select the adapter connected to your players under **CDJ discovery interface**.

Save and restart OLC after changing connection modes. Select your library in BROWSE and load onto a stopped player.

To use a phone, tablet or another computer, open the network address shown in MENU, such as `http://192.168.1.20:8787`. No login or pairing is needed; everyone connected shares the controls and history. Keep this access on your local network.

Keep OLC running, the computer awake and the USB attached while serving music or recording a set.

## Raspberry Pi headless

To use a Pi without a monitor or a logged-in user, use 64-bit Raspberry Pi OS Lite on a Pi 4 or 5. Configure its network connection and enable SSH during OS setup so you can administer it remotely. Desktop automatic login is not required.

Log in once over SSH as the ordinary Pi account that will run OLC. Quit any running OLC desktop app and disable its login-startup option before switching to the service. Download the host `.deb` from the release, then run the following in its folder (replace the version in the filename with your downloaded version):

```sh
sudo apt install ./onelibrarycompanion-host_0.3.0_arm64.deb
sudo loginctl enable-linger "$(id -un)"
systemctl --user enable --now olc-host.service
loginctl show-user "$(id -un)" --property=Linger
systemctl --user status olc-host.service --no-pager
```

Expect `Linger=yes` and an `active (running)` service. [Systemd lingering](https://www.freedesktop.org/software/systemd/man/252/loginctl.html) starts this account's service manager at boot and keeps it running after logout. Enabling `olc-host.service` makes OLC start with that manager. Run the `systemctl --user` commands as the ordinary account, without `sudo`; only installation and enabling lingering need administrator rights. The packaged service restarts OLC after a failure.

If the Pi's hostname is **OLC**, open [http://OLC.local:8787](http://OLC.local:8787) on another device on the same network. For another hostname, use `http://your-hostname.local:8787`. If the hostname does not resolve, find the Pi's LAN address with `hostname -I` and open that address with port `8787`. Configure your CDJ connections and library in MENU/BROWSE. A router DHCP reservation is useful to keep this address stable. Keep access on your local network.

Verify unattended startup:

1. Run `sudo reboot` from SSH. Wait for the Pi to reconnect to the network.
2. Before logging in again, open the remote URL on your tablet or computer. Confirm the UI loads.
3. If using a local music USB, confirm the library is available and test loading onto a stopped player. On a headless Pi, configure the USB to mount at boot at a stable path, readable by the OLC account; mounting it through a desktop session is insufficient. See [Local USB help](local-usb.md).

If the URL does not respond, reconnect over SSH as the same account and check:

```sh
loginctl show-user "$(id -un)" --property=Linger
systemctl --user is-enabled olc-host.service
systemctl --user status olc-host.service --no-pager
journalctl --user -u olc-host.service -b -n 100 --no-pager
hostname -I
```

Check that the Pi has a network connection, the address is current, and any firewall permits LAN access on TCP 8787. Use either the desktop app or this service, not both at once. See the [development guide](development.md#host-configuration) for host configuration.

MENU includes confirmed **Restart OLC** and **Shut down host** controls when
running this service. Shutdown requires passwordless permission for the fixed
command `/usr/bin/systemctl poweroff`. If your Pi account does not already have
it, use `sudo visudo -f /etc/sudoers.d/olc-poweroff` and add the following line,
replacing `your_pi_user` with the account running OLC:

```sudoers
your_pi_user ALL=(root) NOPASSWD: /usr/bin/systemctl poweroff
```

If local USB serving reports that UDP port 111 is occupied by `rpcbind`, check
whether the Pi uses any other NFS/RPC services. On a dedicated OLC Pi without
those services, release the port with:

```sh
sudo systemctl disable --now rpcbind.socket rpcbind.service
```

To stop OLC and disable its automatic startup:

```sh
systemctl --user disable --now olc-host.service
```

If this account no longer needs any user services running without login, also run `sudo loginctl disable-linger "$(id -un)"`. This affects all lingering user services for that account.

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

### Safely removing a USB on the headless Pi

In MENU, use **Unmount USB · volume name**, then confirm. Turn off or disconnect the CDJs first. OLC briefly stops, unmounts the selected external USB volume through UDisks, then starts again. Keep the USB connected until the page says it is safe to remove. A busy volume is never forcibly unmounted; close other applications using it and retry. For USBs with multiple mounted partitions, the button unmounts all of them. USB devices containing system mounts are excluded. The controls appear only on the packaged Linux systemd host; iPad folder access does not provide an OS unmount operation.

The headless USB helper uses non-interactive sudo so it works without a desktop login. The Pi setup account already has passwordless sudo. For a restricted service account, grant only the required UDisks unmount command with `sudo visudo` (replace `your_pi_user`):

```sudoers
your_pi_user ALL=(root) NOPASSWD: /usr/bin/udisksctl unmount --no-user-interaction -b /dev/*
```

The packaged helper independently restricts operations to mounted external USB devices with no system mounts; it never forces an unmount.

### iPad Safari display and waveform diagnostics

Open OLC in Safari, reload after a UI update, and use Share → Add to Home Screen. Launch the new Home Screen icon to open OLC without Safari tabs or its address bar. Standalone support uses the iPadOS 17-compatible Apple web-app meta tag; no OS update or service worker is required. A Home Screen web app can keep its own preferences separately from the Safari tab.

For scrolling stutter, reproduce it in CDJ STATUS, then open MENU → Waveforms → Show waveform performance. The report retains the last two scrolling canvases’ measurements before MENU replaced them. Frame median/P95 and drawing cost measure the display device, while position-jump counts measure deviations from steady predicted playback (including seeks or unreported wraps). These figures do not establish the cause alone. No report is uploaded automatically.

### Updating a Pi while using CDJs

Before replacing the host package or restarting its service, stop CDJs using
tracks served by OLC and verify fresh stopped status. Restarting during playback
can cause an emergency loop; loaded tracks may require reloading afterward.
Browser files can be updated without restarting the host. The MENU restart and
shutdown endpoints enforce the playback check, but direct SSH/systemd/package
operations must perform it separately.
