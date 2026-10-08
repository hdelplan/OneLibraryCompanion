# Set history

OLC keeps its own archive of performances on the host. It does not write to the USB's history. Open SET HISTORY to record, edit, export or delete sets; use the SET HISTORY selection inside BROWSE to reuse a past tracklist as a playlist.

## Record a performance

Choose **Start set**, give the set a name, and perform on the connected CDJs. A track is added after **more than 45 seconds** of uninterrupted normal playback or active looping. Exactly 45 seconds does not qualify. Pauses, cue audition, changing tracks and stale status interrupt qualification; offline previews are excluded. A later qualifying replay becomes another entry, and the two decks qualify independently.

OLC records reported CDJ playback rather than mixer audibility. The host must remain running, but you can change screens or close a remote browser without stopping recording. Playback before starting the set cannot be recovered.

Choose **Finish & save** to retain the set, or **Cancel set** to discard it. A nonempty cancellation asks for confirmation. After an interrupted host session, use **Resume set** or **Finish & save** to handle the recovered recording.

## Edit and manage sets

Name, location and comment save when you leave a field; Enter also saves the name. **Edit tracklist** provides reordering and removal. **Restore original** returns the original recorded order and entries. Exports use the edited order. Changes do not modify the USB or its metadata.

The archive retains track metadata and available artwork after the USB is removed. Missing metadata uses a track-ID placeholder. Set dates and times are stored; individual track timestamps are not displayed.

Swipe a set right-to-left to reveal DELETE, or use its keyboard-accessible delete control. Confirm the named set to remove it. Deleting a past set does not stop another active recording. Existing saved sample-labelled sets remain readable as sample data.

## Import Rekordbox history

Choose **Import history** and a library source. OLC imports complete histories whose names contain an unambiguous `YYYY-MM-DD` date. History entries keep their original order and repeated tracks. Imports with the same identity are not duplicated or used to overwrite your edits.

Undated names such as `HISTORY 001`, invalid dates, empty histories and histories with missing tracks are skipped. If none can be imported, OLC reports the reason. Imported sets have a date but no invented start/end time. This importer uses the legacy Rekordbox history tables; local OneLibrary histories are not decoded.

## Use a past set in Browse

Select **SET HISTORY** in BROWSE, choose a saved set and select the USB you want to use. OLC resolves saved tracks against that source using path, title and artist, with an unambiguous metadata match for older records. It does not trust row IDs across different exports.

The table retains the set order and repeated tracks; you can search, filter and sort it as usual. Missing or ambiguous tracks stay visible with load buttons disabled. Selecting a set restores its saved order. Player-state and source checks apply to every load request.

## Export and share

Export the selected set, the last finished set or all past sets as:

- **Text:** a portable UTF-8 tracklist.
- **CSV:** spreadsheet-ready metadata with quoted fields and spreadsheet-formula protection.
- **PDF:** a paginated, printable tracklist with set details, title/artist, key and rating. Its text is rendered and is not selectable.

**Save file** opens a native save dialog or browser download. **Share** uses supported browser file sharing with a download fallback. **Email** downloads the export and prepares a mail draft; attach CSV/PDF files yourself. OLC does not send email automatically. Finish pending metadata edits before exporting.

## Storage and backup

Sets and copied artwork are stored in the [host data directory](distribution.md#data-and-backups), shared by all clients. Only one host should use a directory at a time. Quit OLC before backing up or moving the complete directory.

The host writes history atomically. A persistence error is shown explicitly and retried while running; corrupt or unsupported history files are preserved rather than overwritten.

## Played-track marks

Track titles in BROWSE turn green after more than 45 seconds of uninterrupted
normal playback or active looping, even when no set is recording. Pauses, cue
audition, changing tracks and stale status reset the qualification timer.
Offline previews are excluded. Marks match the original file path, track ID,
title and artist; they persist across OLC restarts.

Use **Clear played tracks** in the SET HISTORY top banner to reset the marks.
This restarts qualification for currently playing tracks and leaves recorded
sets and exported tracklists intact. Starting, pausing, finishing, cancelling
or deleting a set does not clear the marks.
