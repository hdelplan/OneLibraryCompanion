# Architecture

OLC has one host session shared by a native desktop window and LAN browsers. The host owns the CDJ network connection, library catalogs, local music serving and durable history. Clients render snapshots and submit explicit commands; browser lifetime does not control recording.

## Components

| Component | Responsibility |
| --- | --- |
| `source/core` | Validate ANLZ containers and decode native PWV6/PWV7 waveform data. |
| `source/host` | Axum HTTP/SSE service, CDJ observation, library reading/loading, local audio serving, history and configuration. |
| `source/ui` | Shared React interface, canvas waveform rendering, client preferences and exports. |
| `source/desktop/macos` | AppKit/WKWebView launcher, host lifecycle, native file dialogs and login startup. |
| `source/desktop/linux` | GTK/WebKit desktop launcher and host lifecycle. |
| `vendor/prolink` | Pinned networking/protocol, export reader and serving crates; see its provenance document. |

## Observation and display

`live.rs` and `direct_live.rs` adapt automatic discovery and manual-IP connections into the same model. The owning session receives player status and handles explicit load requests through its existing status socket. `useLiveDecks.ts` consumes SSE updates and uses polling as a fallback; larger analysis and artwork assets are fetched separately.

Snapshot/asset identities include the session, player, source, track and load identity. Async results are checked against that identity before display. Stale status clears live fields. Beat grids, received beat/status timing and permitted motion signals feed the display clock. Unsupported position details remain unknown rather than being inferred as precise scratch tracking.

Waveforms use exported PWV7 detail and PWV6 preview data; DAT beat grids align timing and EXT data supplies cues and phrases. The UI uses cached band geometry, square-root envelopes and optional bass emphasis. Rendering is a visual interpretation, not a firmware-equivalent audio analysis.

## Catalogs and local media

`library.rs` owns generation-scoped catalogs. Linked CDJ sources read legacy `export.pdb` and optional `exportExt.pdb` My Tags. `local_media.rs` reconciles mounted removable volumes every five seconds; `onelibrary.rs` opens local `exportLibrary.db` read-only through SQLCipher. At most three local OneLibrary catalogs are active.

Every library request includes its source generation. Media removal/replacement invalidates it; late reads cannot replace a newer catalog. Parsing and filtering run outside observation. Artwork is lazy, bounded and checked against the source generation. The UI windows the filtered list to bound DOM and artwork work while retaining sorting across all matching tracks.

## Explicit loading and audio serving

`POST /api/live/load` accepts `{source, generation, trackId, target}`, with target 1 or 2. The host resolves the source and rejects stale/missing tracks, disconnected targets and protected playback states. It sends at most one load command. Confirmation checks the player's reported source/track identity; timeout or disconnection yields an unknown result without retrying.

For a host-attached USB, `local_serving.rs` prepares the original file and exported metadata/analysis off the observation loop. The manual-IP session announces virtual source 4 through its existing discovery/status sockets; NFS/dbserver provide file and metadata access. The target must query the source before a load can proceed. Fresh stopped status and source identity are checked again before sending.

Requested tracks receive independent, session-specific wire identities across local libraries. Their original files remain available while another track is selected. Paths are confined to the USB root, and file/analysis sizes are bounded. NFS returns original bytes, including full hardware-sized audio reads within the UDP payload limit, with a dedicated send buffer. There is no transcoding or remote play command.

Platform policy exposes local playback only where the desktop package supplies port-111 access. The Pi installer grants a file capability to the host executable. The Mac package reports local sources as browse-only because it lacks the required access to UDP port 111.

## HTTP interfaces

| Route family | Purpose |
| --- | --- |
| `/api/health`, `/api/app` | Host identity, mode, LAN addresses and saved connection settings. |
| `/api/live`, `/api/live/events` | Live snapshots and SSE stream. |
| `/api/live/analysis/{number}`, `/api/live/artwork/{number}` | Current player assets with matching identity. |
| `/api/live/load` | Explicit checked track selection. |
| `/api/library/sources`, `/api/library/local` | Available catalogs and mounted local library status. |
| `/api/library/{id}`, `/tracks`, `/refresh`, `/artwork/{track}` | Generation-scoped catalog metadata, queries, refresh and covers. |
| `/api/sets`, `/api/sets/import`, `/api/sets/artwork/{name}` | Durable set archive, commands, imports and cached artwork. |
| `/api/analysis`, `/api/capture` | Offline analysis decoding. |

The library subpaths in the table are relative to `/api/library/{id}`. The router in `source/host/src/lib.rs` is authoritative. Default HTTP binding is `0.0.0.0:8787` without authentication. Static UI assets and notices are served from the configured UI root.

## Persistence

`set_history.rs` owns qualification and archive changes. Normal playback/active looping must continue for more than 45 seconds; a pause, track change or observation gap resets the timer. Metadata is copied into recorded events. Edited order is separate from original events so restoration does not depend on the source USB.

History writes use a temporary file, flush, rename and directory flush. Unsupported or corrupt stores are preserved and block writes. Browser display settings and filter presets use local storage; host settings, history and retained artwork use the application data directory. No operation writes history or metadata back to a USB.
