# OneLibraryCompanion 0.5.0

## CDJ configuration profiles

Import saved My Settings from connected media or a file, edit profiles, and
transfer profiles between OLC hosts. Push settings to a stopped CDJ-2000nexus or
optionally apply a default/per-player profile once on connection. Automatic
application is off by default. A sent command requires visual confirmation on
the player. [Configuration guide](cdj-configurations.md).

## Browse, settings and set history

Browse, MENU and Set History use the updated shared layout. Standard metadata
filters and personal My Tag categories have separate ordering controls. Selected
library categories are available directly in settings. Display sizing, waveform
comparisons, audio preview, transcoding and automatic set capture are available
through the shared desktop/browser interface.

## Automatic load diagnostics

Failed or unconfirmed loads save bounded incident reports with recent player
states and file-serving evidence. Reports also record observed serving failures,
file-read latency and overload drops. No audio payloads are saved.
[Diagnostic details](configuration.md#automatic-load-diagnostics).

## Installation

Mac downloads support Apple Silicon and Intel on macOS 13+. Raspberry Pi packages
support ARM64 Raspberry Pi OS Bookworm or later, with desktop and headless setup.
Mac packages are ad-hoc signed and not notarized. Reinstall **Local USB Support**
after updating the Mac app.

Stop players using OLC-served tracks before updating or restarting the host;
loaded tracks may need reloading afterward.

[Install OLC](distribution.md) · [Compatibility](compatibility.md).
