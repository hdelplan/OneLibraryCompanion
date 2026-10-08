# Make OLC suit your setup

Open MENU to connect your players and adjust the display and library filters.

## Navigation

Click **CDJ STATUS / BROWSE** to switch between the two screens. From MENU or SET HISTORY, it returns to your last performance screen. Its text matches the other navigation buttons. BROWSE keeps your previous list position when you return.

## CDJ connections

Choose **Manual IP connections** for players 1 and 2 when using a USB attached to the Mac/Pi. Subnet search helps find their addresses. Mac local playback also requires **Local USB Support** from the application menu.

For linked-CDJ operation, select the network adapter connected to the players under **CDJ discovery interface**. Save and restart OLC after changing connection modes.

To use another device, open one of the network addresses shown in MENU. Everyone connected uses the same players, libraries and set history, without a login or pairing. [Setup guide](distribution.md).

## Display

Adjust waveform zoom, playhead position and color, motion smoothing and band emphasis. Choose elapsed or remaining time, and show or hide track overviews, phrase sections and extra details.

The CDJ STATUS zoom buttons and two-finger pinch change the visible window. Tapping a track's time switches elapsed/remaining time. **Restore defaults** resets display and filter preferences without deleting music or set history.

## Library filters and track size

Choose which filters appear in BROWSE and arrange them in your preferred order. Set the BPM choices and include My Tag categories from your export.

For My Tags, **ANY** accepts any selected tag, **ALL** requires every selected tag and **NONE** excludes those tags. Active filter chips show what is restricting the list; use **Clear filters** to start again.

Choose small, medium or large track rows for more tracks or larger touch targets. Save useful filter combinations as named presets for the selected USB library.

## Audio transcoding

Choose **Automatic**, **WAV 16-bit**, **WAV 24-bit**, **AIFF 16-bit** or **AIFF 24-bit**. Automatic preserves supported source quality while preparing a format the destination can use. Compatible originals are always left unchanged.

OLC shows conversion progress and offers cancellation during preparation. CDJ STATUS identifies the converted output once loaded. The conversion benchmark has been removed. No separate audio software is required. [Local USB and conversion limits](local-usb.md).

## Local USB panel

Open **Local USB** in MENU to check attached libraries, connected players and any loading messages directly in the panel. Select the library itself in BROWSE.

## Offline preview

Open an exported analysis file to inspect its waveform without loading a CDJ. Choose **Show live CDJs** to return to the players.

## Preference storage

Display choices and filter presets are saved separately on each device or browser. Connections and transcoding settings apply to the shared host. History and host settings are stored in the [application data directory](distribution.md#data-and-backups).
