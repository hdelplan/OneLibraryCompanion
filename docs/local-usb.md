# Play from a USB connected to OLC

Connect a OneLibrary USB to your Mac or Raspberry Pi, browse it in OLC and load tracks onto either CDJ. Both players can use different tracks from the same local library. Your original music, artwork and analysis stay unchanged.

## Set up once

- **Mac:** choose **OneLibraryCompanion → Local USB Support… → Open Installer**, approve the macOS installation, then quit and reopen OLC. Repeat after updating OLC.
- **Raspberry Pi:** install both OLC desktop packages and make sure the USB is mounted and readable.
- In MENU, use **Manual IP connections** for physical players **1** and **2**. Both must be on the same local network. Leave player number **4** free for OLC.

[Installation help](distribution.md).

## Browse and load

1. Attach your exported OneLibrary USB to the computer running OLC. Keep its exported folders intact.
2. Select the USB in BROWSE. If it is missing, check **MENU → Local USB** for an explanation.
3. Find a track, stop the destination player and press **CDJ1** or **CDJ2**.
4. If the CDJ has not found the source, press **LINK** and look for your USB's name. Follow OLC's loading message and check the player's selection before trying again.
5. Keep OLC running, the computer awake and the USB attached throughout playback.

The load action selects a track; playback and cueing remain on the CDJ. Check the cue position before playing.

## Audio transcoding for older CDJs

OLC can prepare supported local files such as FLAC and ALAC for players that cannot play them directly. Select the track as usual: OLC converts it to compatible WAV or AIFF before loading and shows progress. **Cancel conversion** stops preparation; it does not stop or unload a track already on a CDJ.

Choose automatic quality preservation or a 16-/24-bit WAV/AIFF output in [Audio transcoding settings](configuration.md#audio-transcoding). Compatible tracks are served unchanged. Converted copies stay on the computer, leaving your USB untouched. Exported waveforms, beats and cues accompany the music.

Conversion is available for supported files and known destination models. It does not convert tracks on USBs plugged directly into the CDJs.

## Switching libraries

OLC can detect up to three attached libraries, but serves **one local library per connection session**. To switch libraries, refresh an export or change an already-loaded track's conversion format, stop both players, disconnect both in OLC and reconnect.

## Things to keep in mind

- Attach the USB to the Mac/Pi running OLC, not to a device only viewing its web interface.
- Local loading currently requires Manual IP connections. Automatic discovery can browse local libraries but cannot load from them.
- Use OneLibrary exports with their original music and analysis folders. Missing or unsupported files cannot be loaded.
- A session supports up to 128 different requested selections. Use OLC to browse the full library; the CDJ's OLC source lists the tracks prepared in that session.
- Conversions are limited to 512 MiB per track, a 2 GiB session cache and two minutes of preparation. OLC reports an error if a limit is reached.

See [compatibility and operating limits](compatibility.md) for player and platform coverage.
