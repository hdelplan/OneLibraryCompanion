# Local USB libraries

OLC reads up to three OneLibrary USBs attached to the Mac or Raspberry Pi host. Each drive must be mounted and readable by the logged-in user, with its original `PIONEER/rekordbox/exportLibrary.db`, music and analysis directory structure intact. OLC checks mounted removable drives every five seconds and adds valid libraries to the BROWSE source selector. It reads the export without modifying it.

## Browse and load

1. In MENU, select **Manual IP connections**, save the connection mode and restart OLC if changing modes. Connect physical CDJs numbered **1** and **2** by their IP addresses. Both must use the same host network interface. Leave player number **4** unused: OLC uses it as the local music source.
2. Attach the USB to the host. On Pi, mount it through the operating system first. Open **LOCAL USB** in BROWSE to check discovery or validation errors, then select the library in the USB selector.
3. Choose a playlist or filter/search for a track. Stop the destination CDJ, then press its **CDJ1** or **CDJ2** load button. Read the result above the track list.
4. On a first connection, the CDJ may need **LINK** before it discovers **OLC LOCAL USB**. Follow the load message, wait for the source to appear, then make a new explicit load request. If the outcome is unknown, check the physical player's selection before trying again.
5. Keep OLC running, the host awake and the drive attached throughout playback. Stop both players before disconnecting the USB or changing the connection mode.

The file is streamed from the USB without transcoding. Exported metadata, artwork, waveform analysis, beats and cues accompany it. Each requested track has an independent serving identity, so loading another track on the other deck does not replace the first deck's audio source.

## Operating limits

- Automatic-discovery mode supports local browsing; local track loading requires Manual IP connections.
- The destination player's audio format, sample rate and bit-depth limits still apply. OLC flags known incompatibilities; it cannot make an unsupported file playable.
- Local sources require OneLibrary `exportLibrary.db`. Linked CDJ libraries use legacy `export.pdb`; an optional configured local legacy export is a separate browse-only source.
- Keep the original exported paths. Missing files, paths outside the mounted volume, changing libraries and invalid databases are rejected.
- The serving session holds up to 128 distinct requested selections. The native CDJ source lists requested tracks; use OLC to browse the full library. Restart the session only after stopping playback.
- The load action selects a track. OLC has no start-of-track/earliest-hot-cue setting; the player's resulting cue position must be checked on the CDJ.
- A USB attached to a web client is not attached to the OLC host. All clients use the host's mounted libraries.

See [compatibility](compatibility.md) for platform validation limits.
