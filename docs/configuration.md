# Connections and preferences

Open MENU to configure the host connection and the display. Host settings are shared by every client; display and library preferences are saved separately in each browser or native window.

## CDJ connections

**Manual IP connections** lets you connect players 1 and 2 by address. Subnet search helps find players on the chosen local subnet. Use this mode on Pi when loading from a host-attached OneLibrary USB. Mac local USBs are browse-only in this package.

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
