# CDJ configuration management

Open **MENU → CDJ configurations → Manage profiles & automatic application** in the normal OLC application. Profiles are saved on the OLC host and shared by browsers using that host.

## Capture and edit

1. On the CDJ, use **MY SETTINGS → SAVE** to save your current choices to its USB or SD card.
2. In OLC, choose the connected source player and media, then **Read saved My Settings**.
3. Rename the imported profile, edit options, and press **Save profile**.

OLC reads `/PIONEER/MYSETTING.DAT` through the player's media service. This imports the saved file, not a live readback of the player's current controls. A direct MYSETTING.DAT file import is also available. File checksums are reported; unknown enumerations and unedited bytes are preserved. Settings from MYSETTING2.DAT and mixer settings are outside this module.

The editor covers 17 core settings for the original **CDJ-2000nexus**: LCD brightness, time display, language, on-air display, jog-ring brightness and indicator, disc-slot illumination, slip flashing, play mode, eject/load lock, Auto Cue and threshold, jog mode, tempo range, Master Tempo, Quantize, and Sync. The profile includes all 32 original My Settings bytes; unsupported fields remain unchanged when editing.

Use **New profile**, **Duplicate**, and **Delete profile** to manage profiles. Export/import OLC profile JSON to transfer a profile between hosts. Exported profiles include the original imported file for preservation; editing changes the settings block used for remote push. OLC does not write edited profiles back to the USB.

## Push

Select a saved profile and press its **Push … to CDJ …** button beside a connected compatible player. The host rechecks player identity, address, profile revision, fresh status and stopped state immediately before sending. Track loads and diagnostics can block a push. Playing, stale, disconnected and unvalidated models cannot receive a configuration through this module.

OLC sends the native 116-byte Pro DJ Link **0x34 Load Settings** command over its existing connection. It reuses an already announced local USB source when ready, otherwise the existing observer identity. Configuration management does not claim an additional player number or change served library identity.

**Sent** means UDP transmission succeeded. A settings-application acknowledgement is not known, so check the CDJ screen. No retries are sent automatically. Native push and reconnect application were observed on the original CDJ-2000nexus in the hardware feasibility test; brightness was the visible test setting. The integrated application and remaining options need normal hardware acceptance testing.

## Automatic connection rules

Automatic application is **off by default**. Choose a default profile for compatible players, optionally assign different profiles to individual players, enable automatic application, then **Save automatic settings**.

- Applies once per connection, after a two-second settling period, including connected players detected when OLC starts.
- Requires fresh stopped status at send time. If busy, playing, stale or otherwise unavailable, the attempt is skipped and recorded; stopping later does not trigger a retry.
- A player must be observed absent for at least ten seconds before a reconnect counts as a new connection. Short network gaps and delayed polling do not cause repeat pushes.
- Saving rules does not push immediately to existing connections. Use the manual push button to apply now.
- Changing or disabling a rule, editing/deleting its profile, or replacing the target connection cancels a pending automatic command.

Discovered player rules follow their MAC identity. Manual Direct IP rules follow the configured IP address; update assignments if those addresses change. Recent commands are retained for the current host run and show manual/automatic attempts separately.

## Storage and compatibility

Profiles and rules live in `cdj-configurations.json` inside the host application data directory (`OLC_DATA` when overridden). Writes are atomic; stale profile/rule revisions are rejected. Unreadable archives are preserved and shown as an error instead of being overwritten.

My Settings is separate from the music-library database: traditional exports and OneLibrary do not change this settings format or native push command. Player media still needs to expose a saved MYSETTING.DAT file for import. Remote rekordbox collection browsing is a separate feature.

Protocol reference: [Deep Symmetry, Loading Settings](https://djl-analysis.deepsymmetry.org/djl-analysis/loading_tracks.html#_loading_settings).
