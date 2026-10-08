# Local USB libraries

OLC reads up to three OneLibrary USBs attached to the Mac or Raspberry Pi host. Each drive must be mounted and readable by the logged-in user, with its original `PIONEER/rekordbox/exportLibrary.db`, music and analysis directory structure intact. OLC checks mounted removable drives every five seconds and adds valid libraries to the BROWSE source selector. It reads the export without modifying it.

## Platform availability

Local browsing and audio serving are included on both platforms. On Mac, choose **OneLibraryCompanion → Local USB Support… → Open Installer**, approve installation in macOS Installer, then quit and reopen OLC. The package installs a small system service that opens UDP port 111 for the matching OLC host executable. The host continues to run as the logged-in user and reads your USB with that user's permissions. Reinstall Local USB Support after updating OLC.

The component authenticates the host using its code signature and provides only the fixed discovery socket; it does not read music or run arbitrary commands. An absent component disables local loading; authorization errors and port conflicts are reported when serving starts. After stopping playback and quitting OLC, run **Remove Local USB Support.command** in the app's `Contents/Resources` folder to uninstall the component. Removal requires administrator approval and preserves music and saved sets.

The Pi host package grants its executable `CAP_NET_BIND_SERVICE` during installation so it can bind port 111 while running as the logged-in user. It does not change the system-wide privileged-port range. Another RPC/NFS service using port 111 prevents OLC from serving local music.

## Browse and load

1. In MENU, select **Manual IP connections**, save the connection mode and restart OLC if changing modes. Connect physical CDJs numbered **1** and **2** by their IP addresses. Both must use the same host network interface. Leave player number **4** unused: OLC uses it as the local music source.
2. Attach the USB to the host. On Pi, mount it through the operating system first. Open **LOCAL USB** in BROWSE to check discovery or validation errors, then select the library in the USB selector.
3. Choose a playlist or filter/search for a track. Stop the destination CDJ, then press its **CDJ1** or **CDJ2** load button. Read the result above the track list.
4. On a first connection, the CDJ may need **LINK** before it discovers **OLC LOCAL USB**. Follow the load message, wait for the source to appear, then make a new explicit load request. If the outcome is unknown, check the physical player's selection before trying again.
5. Keep OLC running, the host awake and the drive attached throughout playback. Stop both players before disconnecting the USB or changing the connection mode.

Compatible files are streamed from the USB unchanged. Known incompatible local files can be converted into a local WAV/AIFF cache before loading. Exported metadata, artwork, waveform analysis, beats and cues accompany it. Each requested track has an independent serving identity, so loading another track on the other deck does not replace the first deck's audio source.

## Unresolved loading issues

- **First load:** the initial local USB request may not load the track while the CDJ has not discovered OLC’s source. The LINK step above may help, but is a workaround rather than a fix. Check the physical player before sending a new request.
- **Incorrect start position:** a loaded track can land at an unexpected position, including a saved cue, instead of the intended start. Check and set the cue position on the CDJ before playback; OLC does not reliably establish that position.

Both issues remain unresolved.

## Operating limits

- Automatic-discovery mode supports local browsing; local track loading requires Manual IP connections.
- The destination player's audio format, sample rate and bit-depth limits still apply. OLC shows amber **TRANSCODE NEEDED** for known convertible local tracks. MENU selects the PCM target; unsupported decoders and unknown conversion destinations remain unavailable.
- Local sources require OneLibrary `exportLibrary.db`. Linked CDJ libraries use legacy `export.pdb`; an optional configured local legacy export is a separate browse-only source.
- Keep the original exported paths. Missing files, paths outside the mounted volume, changing libraries and invalid databases are rejected.
- The serving session holds up to 128 distinct requested selections. The native CDJ source lists requested tracks; use OLC to browse the full library. Restart the session only after stopping playback.
- The load action selects a track. OLC has no start-of-track/earliest-hot-cue setting; the player's resulting cue position must be checked on the CDJ.
- A USB attached to a web client is not attached to the OLC host. All clients use the host's mounted libraries.

See [compatibility](compatibility.md) for platform validation limits.


## Local audio conversion

On Mac and Pi hosts with local serving available, pressing an amber
track's CDJ load button converts the complete audio file first, then rechecks
that the destination is connected, fresh and stopped before sending the load
command. The UI reports conversion progress, cache reuse and elapsed time.
**Cancel conversion** stops preparation without sending a load command when the
worker observes cancellation; no remote unload or stop is sent if conversion has
already completed. Linked CDJ-mounted USBs are not transcoded by this feature.

The original USB, database, artwork and analysis files are never written.
Converted files use classic integer PCM WAV or AIFF headers and independent
serving identities for each variant. Existing beat grids and cues retain their
time coordinates. Cache files live in the host data directory, with a 512 MiB
per-track and 2 GiB session limit, and remain pinned while the serving session
uses them. Unused cache entries may be evicted; old process caches are cleaned
at startup. A conversion has a 120-second worker limit. Keep the USB attached
and OLC running as before. ALAC is decoded to PCM because the current serving
metadata has no verified ALAC container code, even for newer players.

See [conversion configuration and measurements](configuration.md#audio-transcoding-and-benchmarks).
