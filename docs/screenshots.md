# Using OLC

Use **CDJ STATUS / BROWSE** to switch screens; its font matches the other navigation buttons. BROWSE remembers the list position when you return.

Follow the mix, choose your next track and keep a record of the set. Playback and cueing stay on your CDJs.

## CDJ Status — follow the mix

Use the two-deck view to compare timing and track structure. Zoom the waveforms, watch approaching cues and phrases, and tap the time display to switch between elapsed and remaining time. The information button opens more track details.

![CDJ status](screenshots/cdj-status.png?v=2)

Converted local tracks show their output format and bit depth on one line in the left panel.

Fine waveform tracking is limited on a paused non-master player. Use the CDJ's display and audio for precise cueing. [Tracking limits](compatibility.md#main-limitation-paused-non-master-waveform-coupling).

## Library — choose the next track

In BROWSE, choose a USB and playlist, then search or combine filters. Save useful filter combinations, use key highlighting to find compatible selections and check played-track marks before repeating a track. Currently playing track titles are bright green on either deck; previously played titles use softer green independently of set recording. Conversion-capable load buttons remain white.

Press **CDJ1** or **CDJ2** to load a stopped player.

![Library](screenshots/browse.png)

A USB connected to the Mac/Pi can also supply music to both players, including supported tracks that need conversion. [Local USB and transcoding](local-usb.md).

## Browser audio preview

In BROWSE, choose **PLAY** in track details or hold a title to open the player. Use the round green play/pause control and click the overview waveform or use the seek slider to move through the track. Close the popup to stop preview playback. Audio comes from the browser device, so a headless Pi can serve tracks to a tablet or computer.

The popup shows artwork, time, BPM, key, genre, My Tags, comments and library color. Exported phrases and saved cues appear when available. Local AIFF/AIFC files receive a temporary WAV playback copy; linked CDJ audio depends on browser codec support. Preview playback does not enter set history.

## Set History — record and reuse a set

OLC captures playback automatically and saves the session after more than five minutes without playback. Use **Split set here** to separate performances or **Finish set** to save immediately. Edit the tracklist, add notes and export text, CSV or PDF. In BROWSE, select **SET HISTORY** to use a past performance as a playlist. [Set history guide](set-history.md).

![Set history](screenshots/set-history.png)

*These 1280 × 800 screenshots illustrate active sessions. CDJ status uses the real exported waveforms, beat grids, cues and phrases for Hail From Mali and What You Want. Playback and connection states are illustrative; the library uses example data.*

[Install OLC](distribution.md) · [Display and filter preferences](configuration.md)
