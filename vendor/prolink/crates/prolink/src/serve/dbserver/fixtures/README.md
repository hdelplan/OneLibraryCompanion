# Native cue regression fixtures

`native-russia.*` and `native-danz.*` come from the user's build-51 native
cue reports captured on 8 October 2026 from CDJ2, a CDJ-2000nexus running
firmware 1.30, with DJ RV plugged directly into the player:

- `olc-native-cues 2 - Russia .json`: Russia (Nightmares on Wax Remix), USB ID 1687.
- `olc-native-cues - Danz.json`: Danz Danz (Original Mix), USB ID 3.

The `.dat` and `.ext` files contain the corresponding report's verbatim PCOB
and PCO2 tag bytes, wrapped in a minimal 12-byte PMAI header with the new
file length. They exclude audio, waveforms and unrelated analysis tags.
The `.reply` files are decoded from `nativeReply.reencodedMessageHex`.
Their message envelopes were reencoded by the diagnostic; both binary cue
blocks are unchanged native response bytes, not generated expectations.

The regression parses the USB tags and compares the entire generated reply
against the captured reply using its transaction ID. Both native replies
contain memories in stored order, then hot slots A/B/C in letter order.
Memory and hot entries at identical positions remain separate.

`russia-cues.anlz` predates these captures and contains the verbatim PCO2
cue tags from the DJTT Russia export, also under a minimal PMAI header.
