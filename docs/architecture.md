# Architecture

For the current native desktop launchers, packaged data paths and LAN access, see [Distribution](distribution.md). Shared routing is in `source/host/src/lib.rs`; desktop startup is in `distribution.rs`.

## Boundaries

- `source/core`: validates ANLZ containers and decodes native PWV6/PWV7 bands. No audio FFT, network or player control.
- `source/host`: loopback Axum HTTP host and blocking-worker offline decoder. `offline.rs` owns optional capture/library sources and exact unique-path metadata matching. The library is loaded once at startup.
- `source/ui/src/model.ts`: shared UI analysis, waveform, track and offline deck contracts.
- `api.ts`: HTTP access. `main.tsx`: app state, navigation and offline preview selection.
- `Configuration.tsx`, `StatusDisplay.tsx`, `TrackCard.tsx`: presentation. `Signal.tsx`: canvas lifecycle. `waveform.ts`: drawing and bass display tuning. `settings.ts`: validated local preferences.
- `vendor/prolink`: pinned upstream reader subset, with provenance and license.

`source/host/src/live.rs` adapts Prolink discovery and status into live snapshots. `useLiveDecks.ts` polls status every 200 ms and fetches larger waveform assets separately. Offline scrubbing remains distinct from hardware seeking. Snapshot and asset keys include observer session, player, source slot, track ID and load epoch, so old asynchronous results cannot attach to a newly loaded track.

Live mode announces an observer with status emission disabled. No serving or play/stop APIs are started. Explicit library load requests are handled by the existing observation session. Missing or >2-second-old status clears live fields; discovery eventually labels the peer disconnected. USB/SD reads have a 35-second total timeout. Failed reads are surfaced; reload the track or restart the host to retry. Assets are bounded and held in memory. The first two discovered players by number are displayed; expanded deck selection is deferred.

## HTTP contract

- `GET /api/health`: configured mode (`offline-read-only` or `live-monitor`), enabled live flag and startup error. Enabling live mode does not itself mean a player is connected.
- `GET /api/live`: enabled flag, error and player snapshots with connection freshness, source identity, status fields and nullable position.
- `GET /api/live/analysis/{number}`: current connected player's asset and matching key, or 404 while unavailable. No old-track fallback.
- `GET /api/capture`: decode the configured local capture; 404 if none is configured or it cannot be read.
- `POST /api/analysis`: raw ANLZ bytes, up to 16 MiB. Invalid analysis returns 400; oversized request returns 413.
- Successful analysis: `detail`, `preview`, and nullable `track`. Waves include tag, normalized band columns, raw columns, normalization and nullable sample rate. Track fields are defined in `model.ts`.
- Unmatched or ambiguous PPTH paths return null metadata; filenames are never used as identity.

The LIBRARY browser uses separate, generation-scoped routes:

- `GET /api/library/sources`: known sources, availability, generation, state, errors and track count.
- `POST /api/library/{id}/refresh`: start/deduplicate a read; never writes USB contents.
- `GET /api/library/{id}?generation=N`: tree, facets, My Tag definitions and export fingerprint.
- `GET /api/library/{id}/artwork/{track}?generation=N`: lazy JPEG/PNG artwork, or 404. A catalog holds at most 32 cached images (1 MiB each), shares in-flight reads by path, and reuses one NFS client; global read concurrency is two. Results are checked again against the source generation after reading. Local artwork requires a full PIONEER/rekordbox directory layout and stays within that volume.
- `GET /api/library/{id}/tracks?generation=N&...`: filtered/sorted track metadata, total count, and either all matching tracks (`all=true`, used by the UI) or a bounded API page (default 50, max 100). The UI windows rows in one continuous scroll surface to bound DOM and artwork work. Filter parameters include `playlist`, `q`, JSON-array categorical selections, `rating`, BPM/year/duration/date ranges, tag IDs plus `tagMode`, and JSON metadata `rules`. Invalid/unready generations return 409.

`library.rs` owns source discovery state and Arc-backed catalogs; `live.rs` feeds it mounted slot presence from the existing monitor. Ejection/status gaps invalidate the generation and discard the catalog. Background read completions are checked against the current generation. Reads are bounded and database parsing/filtering run outside live observation. Live asset reads reuse a ready catalog when one is available; before a browser catalog is ready, the existing per-track read path remains as a fallback. No second monitor or network session is started.

The old singular `/api/library` route remains absent. Static UI files come from `source/ui/dist` beneath the configured project root.

Live position uses the status packet's one-based beat index with the source `.DAT` beat grid and recent beat phase while playing. The display labels time approximate: a paused cue between beats cannot be placed exactly, and seek/sub-beat alignment still needs hardware comparison. MT, pitch range and quantize are not decoded and remain unknown. A missing `.2EX` yields metadata with an explicit unavailable waveform message, not synthesized three-band audio analysis.

## Waveform interpretation

PWV7 detail uses 150 samples/sec and a 127-height domain. PWV6 preview uses a provisional 255-height domain and has no inferred detail sample rate. Raw analysis is preserved. RX3 colors and weighting are display choices, not an exact reproduction of Pioneer firmware. Bass emphasis applies a 90 ms visual release constrained by the original column peak, reduces amber coverage and resets on silence. Overview duration is supplied separately.


## Explicit USB track loading

`POST /api/live/load` accepts JSON `{source, generation, trackId, target}` with target 1 or 2. Local exports and SD catalogs cannot be loaded. The browser never supplies network addresses or source player numbers. The handler queues into the existing observer, validates the source generation and track membership, checks mounted source USB and fresh target telemetry (<1 second), and sends at most one encoded command through the monitor's existing UDP 50002 socket. Loading/unknown/stale target states are rejected; a target already on the requested source/track is not reloaded.

Playing, looping (including paused loops), cue audition and other active transport states are rejected immediately before sending. There is no confirmation override. The UI disables protected buttons with a red border; server checks are authoritative. No stop command is sent.

A pending request is exclusive per target. Matching fresh source/USB/track/type and state 3–6 must persist for 500 ms before `reported`; this means the CDJ reports the selection, not proof of decoding or audible playback. A disconnect or 12-second timeout returns `unknown`; commands are never retried automatically. The UI also treats a lost HTTP response as unknown. The existing monitor remains the sole status receiver, and rendering/status polling continue during pending loads.


Library rendering is memoized on load-relevant player state, not position packets. Inactive track rows and artwork are unmounted, with browser selection/filter state retained. Source labels identify the player and slot, for example CDJ1 USB.
