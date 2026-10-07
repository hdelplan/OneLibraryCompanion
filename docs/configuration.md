# Configuration

Packaged OLC settings, LAN access and environment aliases are documented in [Distribution](distribution.md). The desktop default is now `0.0.0.0:8787`; desktop offline preview is in MENU.

## Display preferences

Configuration is saved in browser local storage under `pioneercompanion.display.v1`. Corrupt or invalid fields fall back to defaults. Retired renderer settings are ignored; remaining preferences are retained.

| Option | Default | Choices |
| --- | --- | --- |
| Playhead position | One third from left | One third, center |
| Playhead color | Red | Red, white |
| Visible window | 8 seconds | 2, 4, 8, 16, 32 seconds |
| Three-band rendering | Slight bass lift | Native balance, native with 15% bass lift |
| Primary time | Remaining | Remaining, elapsed |
| Overview | On | On, off |
| Phrase analysis | On | On, off |
| Additional details | On | On, off |

Restore defaults resets display preferences. Preview files and scrub positions are session-only. Preferences belong to this browser origin, so a different browser, port or device has separate settings.

## Host environment

| Variable | Default | Purpose |
| --- | --- | --- |
| `PIONEER_COMPANION_ROOT` | Current directory | Root containing `source/ui/dist` |
| `PIONEER_COMPANION_CAPTURE` | None | File opened by Open saved capture |
| `PIONEER_COMPANION_LIBRARY` | None | Optional Rekordbox `export.pdb` for metadata and LIBRARY browsing |
| `PIONEER_COMPANION_INTERFACE` | None | Opt-in live monitoring on the named network interface |
| `PIONEER_COMPANION_BIND` | `127.0.0.1:8787` | HTTP listen address; use `0.0.0.0:8787` for local-network access |

Use absolute file paths. Offline waveform metadata uses the startup library and requires a host restart after changes. The LIBRARY browser rereads its catalog with **Refresh USB**; captures are read on demand. Open analysis file works without a configured capture. For another device on the same network, set `PIONEER_COMPANION_BIND=0.0.0.0:8787`, then open `http://<Mac LAN IP>:8787` in Safari. The HTTP interface has no authentication.

When live monitoring is enabled, the status screen uses live data by default. Loading an offline preview switches the display to offline; **Show live CDJs** switches back. This selection is session-only. Network monitoring continues while previewing offline data. An interface change requires a restart.

## Waveform motion and phrases

Status changes are pushed over SSE to a display-frame clock. Backwards beat movement, search/scratch states and larger seeks re-anchor immediately. Position remains a beat-grid estimate on older CDJs; small sub-beat scratches cannot be reconstructed from unavailable data.

The overview is 168 logical pixels tall and uses additive native bands scaled consistently across the track. PSSI phrases are read from the USB/SD `.EXT` companion and aligned through the `.DAT` grid. Colors follow Rekordbox mood and phrase variants. Standalone offline waveform uploads do not include companion phrase data.


## Library browser

LIBRARY lists freshly observed mounted USB/SD sources without requiring a loaded track. It reads legacy `export.pdb` and, independently, `exportExt.pdb` for My Tag categories and assignments. The configured local database appears as LOCAL EXPORT and its extension is read from the same directory. Newer OneLibrary databases are not supported.

Choose a source, navigate playlist folders, and combine filters. Color uses the exported label (including custom names); an empty label appears as No color. Rating supports minimum stars or Unrated; an exact rating is available through Advanced metadata rules. My Tags supports ANY, ALL and NONE, and unavailable tags are never treated as an empty tag set. Facet counts describe the whole USB; the results count reflects all active filters.

Advanced rules combine with AND and cover all scalar metadata exposed by the browser, including comments, composer, dates, play count and technical fields. Date comparisons expect YYYY-MM-DD, duration uses seconds and file size uses bytes. Track selection shows exported metadata. Loading requires a separate **Load to CDJ1** or **Load to CDJ2** click.

Named filter presets are saved in this browser's local storage, scoped to source and export fingerprint, including tag/playlist selections. An export change requires recreating the preset rather than reusing potentially different IDs. Switching between app screens retains browser filters and position; replacing or refreshing media clears tag/playlist selection. Each database is capped at 64 MiB, an overall read times out after 60 seconds, and optional extension reads have a 15-second timeout. Use Refresh USB to retry a failed read. Target CDJ media lifecycle and My Tag correspondence remain hardware validation items.


Configuration → Library filters controls which filters are shown and their order. Album is omitted from the filter bar; album metadata remains in details and search. Advanced rules are hidden from the main screen. With automatic My Tag categories enabled, the browser uses the USB's actual category names and values. Alternatively, select individual category filters and move them with the up/down buttons. Values within one category support ANY/ALL/NONE; different category filters combine with AND. Hidden active filters remain visible as removable chips. Click a column heading once for ascending order, again for descending; sorting applies to all matching tracks across the complete scrollable result list. BPM menus contain integer options bounded by Configuration → Library filters (default 115–130); Any leaves that bound unset. Clicking outside an open filter closes it.

The load buttons require a selected track from a live CDJ-mounted USB and a connected target. A playing/active target produces a protection dialog with Cancel focused. **Load anyway** authorizes replacement; the server rechecks current status and source identity. Loading may start playback according to hardware settings. A reported selection is not proof of decoding success; inspect the physical CDJ, especially after an unknown result. No automatic retries are sent. Live load integration and multi-player behavior still need hardware validation.

Artwork appears between Color and Title as a small thumbnail, and at the upper left of the selected track's details. Covers load on demand from the USB. Missing covers use a music-note placeholder. A standalone copied export.pdb has no local artwork; local covers require the exported volume's original PIONEER directory structure.
