# Connections and preferences

Open MENU to configure the host connection and the display. Host settings are shared by every client; display and library preferences are saved separately in each browser or native window.

## CDJ connections

**Manual IP connections** lets you connect players 1 and 2 by address. Subnet search helps find players on the chosen local subnet. Use this mode when loading from a host-attached OneLibrary USB. Mac requires installing Local USB Support from the application menu.

**CDJ discovery interface** selects a network adapter for automatic discovery. Save the mode and quit/reopen OLC to apply it. A configured `OLC_INTERFACE` takes precedence over this saved choice. Use the adapter connected to the CDJ network.

MENU lists the host's LAN URLs. Any device that can reach one can use OLC without a login or pairing. Connected clients share player access, libraries and set history. See [installation and networking](distribution.md).

## Display

| Setting | Purpose | Default |
| --- | --- | --- |
| Jog smoothing | Trade immediate motion updates for a smoother display: off, 20, 50 or 100 ms | Off |
| Playhead position | Place the current position one third across or in the center | One third |
| Playhead color | White or red marker | White |
| Visible time window | Show 2–32 seconds in two-second increments | 8 seconds |
| Three-band rendering | Balanced bands or additional low-frequency emphasis | Bass emphasis |
| Primary time | Remaining or elapsed time | Remaining |
| Overview waveform | Show whole-track context | On |
| Phrase analysis | Show exported track sections | On |
| Additional track details | Show supporting track metadata | On |

The CDJ STATUS zoom buttons also change the visible window. Tapping a track's time changes the primary time mode. **Restore defaults** resets display and library-filter preferences; it does not delete history or change the USB.

## Library filters and track size

Choose visible filters and their order under **Library filters**. Available filters cover star rating, BPM, color, key, genre, artist, record label, format, My Tags and date/year/duration ranges. Set minimum and maximum BPM menu options here; selecting Any in BROWSE leaves that search bound unset.

Enable automatic My Tag categories to use names and values from the selected export, or choose individual categories. ANY matches at least one selected tag, ALL requires every selected tag and NONE excludes them. Different categories combine with AND. Filters hidden from the toolbar can still be active; remove their chips or use **Clear filters** to clear them.

In BROWSE, select small, medium or large rows for 16, 12 or 8 visible track rows. Clicking a sortable column heading switches ascending/descending order across the complete filtered list. Named presets retain the selected filters and playlist for a particular export. When that export changes, recreate the preset so it cannot use unrelated playlist or tag IDs.

## Offline preview

Open an exported analysis file from MENU, or use a configured saved analysis capture. Previewing switches the displayed deck to saved data; it does not seek, load or play a physical CDJ. **Show live CDJs** returns to live monitoring. Monitoring continues while previewing.

## Preference storage

Display settings, filter presets and row size belong to the client. A different browser, host IP or port has separate storage; a native window also has its own storage. History and host connection settings reside in the [host data directory](distribution.md#data-and-backups). Invalid preference values fall back to defaults.

Environment configuration and precedence are documented in the [development guide](development.md#host-configuration).


## Audio transcoding and benchmarks

The Mac/Pi converter runs inside the host, without an FFmpeg installation.
MENU → **Audio transcoding** selects Automatic, WAV 16-bit, WAV 24-bit,
AIFF 16-bit or AIFF 24-bit. This preference is saved in the host data directory
and shared by its web clients. Display “Restore defaults” does not reset it.

Compatible originals remain unchanged. For known incompatible local tracks,
Automatic outputs WAV, preserving a 16-bit source and otherwise using 24-bit.
44.1/48 kHz are preserved; 88.2/176.4 kHz become 44.1 kHz; other unsupported
rates become 48 kHz. Mono is duplicated to stereo. Multichannel inputs are
rejected. Rate conversion uses an antialiasing filter with delay compensation;
bit-depth reduction uses TPDF dither. PCM output clamps out-of-range samples
and reports the count. MP3/AAC encoding is not supported.

The **Conversion benchmark** uses a track from a host-attached OneLibrary USB,
always bypasses the conversion cache, and never loads or plays a CDJ. Choose a
target, search/select a track and press **Run benchmark**. Repeat the same track
and target on the hosts being compared. Results include elapsed conversion time, actual
output size/rate/depth, source properties and clipping count; **Share / save
results** exports JSON with engine, OS and architecture. USB reads and local
writes are included; OS file caches are not flushed, and timing does not include
player discovery or NFS transfer. Real load responses separately show total
request-to-CDJ-status-confirmation time, which is not proof of audible playback.

The Mac benchmark works without installing Local USB Support. Loading the
converted file onto a CDJ requires that component. Physical converted-file
playback and cue/loop alignment have not yet been validated on the actual CDJs.
