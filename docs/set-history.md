# SET HISTORY

Start a set explicitly and use **Finish & save** when done. **Cancel set** discards the active set without adding it to history. Empty sets cancel immediately; a set with any original recorded tracks asks for confirmation, even if its visible tracklist was cleared. Past sets cannot be canceled. The host records even when a different tab is open or the browser disconnects. The host must remain running; iPad background suspension is not certified. An interrupted active set remains in history and requires **Resume set** or **Finish & save** after restart.

## Qualification and editing

- A fresh CDJ status must report normal playing or actively looping continuously for **more than 45 seconds**. Exactly 45 seconds does not qualify. Cue audition and offline previews do not qualify.
- Pausing, changing tracks, stale/disconnected status or a recorder observation gap exceeding two seconds starts a fresh timer. Timing uses a monotonic clock, independent of wall-clock corrections. Capture starts when the host observes playback while the set is active; it cannot recover playback before monitoring began.
- Each qualifying uninterrupted run is recorded once. A later qualifying replay is a separate entry, including playback on a different deck. Concurrent decks qualify independently, with ordering by observed start order (deck number breaks ties).
- This records CDJ playback, not confirmed mixer audibility. Track timestamps are deliberately absent. Set start/end date and time are retained.
- Set name, location and comment save on leaving the field; Enter also saves the set name. New recordings start as `Live set` until renamed. The session label is automatic: `CURRENT SESSION` for an active set, `SAMPLE SESSION` for generated demo sets, and `SAVED SESSION` for finished or imported real sets. Export is disabled while these edits are unsaved. Up/down and remove edit a separate list of event IDs; **Restore original** restores the original event order and removed entries.
- Metadata is copied into history. Missing metadata is shown as `Track #ID` and can be filled if it arrives during that playback run. Artwork is copied when available; unavailable artwork has a placeholder. Metadata remains available after removing the USB.

## Storage and reuse

`set_history.rs` contains the versioned model, monotonic qualification state machine, commands and durable host store. The React screen uses `/api/sets`; it does not own recording or persist set data in browser storage. The existing native Rust service can reuse the same model and routes.

Desktop/RPi defaults to `<project>/.local/app-data/`, or the directory specified by `PIONEER_COMPANION_DATA`. The embedded iPad service uses `HOME/Library/Application Support/PioneerCompanion`. Use a persistent writable directory on RPi, outside temporary filesystems. Only one host process should own a data directory.

`history.json` contains schema version 1, active-set identity, copied track metadata, original events and edited order. Every change writes a temporary file, flushes it, renames it into place, then flushes the containing directory. A failed write remains in memory with an explicit error and retries while the host runs. Corrupt or unsupported data is preserved and blocks writes. Artwork is in the adjacent `artwork/` directory. There is no USB writing or USB sync.

## Import and samples

**Import history** uses the existing catalog service without starting another CDJ monitor. It imports complete Rekordbox histories with an unambiguous `YYYY-MM-DD` date in their name on or after **2026-03-01**. Their time is displayed as unavailable. Undated `HISTORY 001` records are not assigned invented dates. The currently decoded history tables have names and ordered track IDs but no reliable timestamp field; other date formats and history synchronization data remain unsupported.

If no usable dated histories exist, create three explicitly labelled sample sets from the `MAX4.0` playlist. Each contains 20–30 unique randomly selected tracks. Every adjacent pair uses the same Camelot key, a neighbouring number with the same letter (including 12/1), or the same number with opposite letters. Unknown keys are excluded. Fail explicitly if a valid sequence cannot be built. Sample dates/duration are illustrative. Re-importing does not duplicate existing sample/import identities or overwrite edits.

The main saved export inspected on 2026-10-03 contained 2,160 tracks, zero history playlists and 77 MAX4.0 tracks. A second local review export contained three one-track histories named HISTORY 001–003, without usable dates. Three samples (23, 23 and 26 tracks) were created in the ignored local app data. These personal library records are not included in source control.

Read-only inspection and local initialization:

```sh
cargo run -p pioneer-companion-host --example inspect_history -- /path/export.pdb
cargo run -p pioneer-companion-host --example import_set_history -- /path/export.pdb /path/app-data
```

The CLI initializes metadata only; importing through the UI also attempts to cache artwork while the source is available.

## Export and sharing

Export the selected set (including the current draft), last finished set or all past sets as UTF-8 text, CSV or PDF. Exports follow edited order. CSV quotes multiline fields and protects spreadsheet formula prefixes. PDF uses paginated high-resolution rendered Unicode text; it is printable but its text is not selectable. It contains sequence, title/artist, key and numeric star rating, plus set details.

**Save file** downloads the export. **Share** invokes file sharing when supported, with a download fallback. **Email** downloads the file and opens a mail draft; text can be included in the body, while CSV/PDF attachments must be added by the user. No email is sent automatically. Actual native share-sheet/download integration in the iPad WKWebView remains a device-validation item.

## API

- `GET /api/sets`: archive, recording state, pending qualification timers and persistence error.
- `POST /api/sets`: `start`, `resume`, `finish`, `cancel`, `metadata`, `move`, `remove`, `restore`. Mutations are serialized in the host and acknowledge durable persistence or return an error.
- `POST /api/sets/import`: `{source, generation}` from the catalog service; validates the source generation and imports or creates samples. Artwork caching continues afterward.
- `GET /api/sets/artwork/{name}`: locally retained raster artwork.

## Validation

Tests cover the strict duration boundary, pauses, track changes, observation gaps, replays, multiple decks, editable versus original order, recovery, invalid storage and exports. Browser checks cover navigation, editing, persistence, start/finish and file downloads. Live CDJ playback qualification, native iPad sharing and recording under iPad lifecycle changes still require hardware validation.

## Library access and deletion

The Library toolbar defaults to playlists. Its `PLAYLISTS / SET HISTORY` toggle lists locally saved past sets as playlist-style entries with track counts. Selecting a set reuses the Library table, search, metadata/My Tag filters, sorting, artwork and CDJ load controls. Set order (including repeats) is the default; selecting a set restores that order after sorting. The dedicated SET HISTORY screen remains responsible for recording, editing, export and deletion. Nested playlist folders continue to use child navigation, a parent-folder button and a breadcrumb path.

For playback, saved tracks are resolved against the selected USB using file path, title and artist when available. Older histories use an unambiguous title/artist match. Saved rekordbox row IDs alone are never trusted across USB exports. Missing or ambiguous tracks remain visible with disabled load buttons. The standard source-generation and player-state checks still govern loading. Newly recorded/imported tracks persist their file path; older archives remain readable.

Swipe a current or past set right-to-left to reveal red DELETE. Completing the swipe opens a shared HTML confirmation dialog with the set name; Cancel/Escape preserves it. Partial or vertical gestures do not delete anything. Keyboard users can focus the entry and press Delete, or focus its delete button. The backend requires explicit confirmation, persists the removal before cleaning up artwork, and preserves an unrelated active recording when a past set is deleted.
