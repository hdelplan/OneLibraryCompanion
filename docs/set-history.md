# Keep and reuse your set lists

OLC records a tracklist while you perform. Saved sets stay on the Mac/Pi and can be edited, shared or used as playlists without changing your USB library.

## Automatic capture

Keep OLC running while you perform. Playback is captured on the host automatically, without pressing Start set. Switching screens or closing a remote browser does not stop capture.

A track qualifies after **more than 45 seconds of continuous playback or looping**. Brief cue auditions do not qualify. A short pause (up to 15 seconds) after qualification continues the same entry; later replays can create another entry. Track entries retain observed start timestamps and repeats.

A session saves after **more than five minutes without playback on any observed deck**. A track playing for six minutes, fifteen minutes, or longer keeps the same session open. The end time reflects last observed playback, excluding the waiting period. Empty sessions are discarded.

Use **Split set here** to mark a DJ changeover or another boundary without silence. **Finish set** saves immediately and ignores the same continuing playback until it stops or changes. **Join previous set** joins adjacent captured sessions, preserving tracks and edits. Rename the set and add location or notes whenever convenient. Canceling a current session discards it; capture remains automatic for later playback.

Sessions and playback activity are persisted. Capture resumes after a short host restart without duplicating the same still-playing track. A restart after a gap longer than five minutes closes the interrupted session. If deck observation disappears for more than five minutes, the saved session notes that its boundary is uncertain. OLC cannot recover playback that happened while the host was off.

The record follows CDJ playback; it cannot tell whether a track was audible through the mixer. Headphone preparation, practice and sound checks can therefore appear in history. Browser audio preview is not CDJ playback and is excluded. Remove unwanted entries or sessions afterwards.

## Edit and manage sets

Use **Edit tracklist** to reorder or remove entries, or **Restore original** to return to the recorded order. Saved track details remain available after removing the USB. Swipe a past set to reveal DELETE, or use its delete control, then confirm.

## Use a past set in Browse

Choose **SET HISTORY** inside BROWSE, select a saved set and choose the USB you want to use. OLC finds the matching tracks so you can search, filter and load them again. Tracks it cannot match confidently stay visible but cannot be loaded.

## Export and share

Export the selected set, the last finished set or all past sets as **text**, **CSV** or **PDF**. Use **Save file** to download, **Share** where supported, or **Email** to prepare a draft. Attach CSV/PDF files to the email yourself.

Exports use your edited order. PDF tracklists are printable; CSV is useful for spreadsheets.

## Import Rekordbox history

Choose **Import history** and a supported library source. OLC imports complete histories with a date such as `2026-10-08` in the name and skips duplicates. Undated histories such as `HISTORY 001`, incomplete histories and local OneLibrary histories cannot be imported.

## Played-track marks

BROWSE immediately shows a track playing on either deck in bright green. Previously played titles use softer green.

BROWSE marks previously played tracks in green after more than 45 seconds of continuous playback, even without recording a set. These marks survive restarting OLC.

Use **Clear played tracks** in the SET HISTORY top banner to reset them. This leaves saved sets intact; starting, automatically saving or finishing a set does not clear the marks.

## Storage and backup

Quit OLC and back up the complete [application data directory](distribution.md#data-and-backups) to keep your saved sets and artwork. All clients connected to that host share the same archive.
