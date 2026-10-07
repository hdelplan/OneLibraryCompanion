# Source credits and research references

OLC thanks the authors and contributors of the following projects. This records the sources documented in this workspace; inclusion as a research reference does not mean that its code or assets are bundled.

## Incorporated or adapted work

| Source | Use in OLC | Attribution and license text |
| --- | --- | --- |
| [usr-ein/prolink](https://github.com/usr-ein/prolink) | Vendored Rust Pro DJ Link networking/protocol, Rekordbox readers and locally modified integration | GPL-3.0-only; see Prolink-PROVENANCE.md, vendored LICENSE files and CARGO-NOTICES.txt |
| [NauticMixxx v1.0.0](https://github.com/nauticsoftware/NauticMixxx/tree/v1.0.0), commit 4a72826059d2b0041057c94e9d65bc82fd50c9fa | 1280×800 UI geometry and waveform summary conventions; skin.xml, overview.xml, waveform.xml, deck.xml, library.xml, rx3_mode_strip.xml, topbar.xml, style.qss and patches 0001/0004/0006/0007/0010 informed the implementation | Repository GPL v3 text and original third-party notices are retained alongside this file. The upstream Mixxx base is GPL-2.0-or-later; upstream skin provenance credits ntamas94/pioneered-by-ntamas and BiteDJ/DeckShark. No logos, screenshots, artwork or proprietary fonts were copied. |
| [nsaintot/cdj3k-mods](https://github.com/nsaintot/cdj3k-mods/tree/e011d7342b8fd9557c699a331d3c8c9f705d4e6e/package/deck) | Band-set waveform algorithm, nested palette and codec research from mods/wave/{codec,paint}.c, wave.h, mods/theme/wave.c and test_wave_codec.c | MIT OR Apache-2.0; MIT text retained in cdj3k-mods-LICENSE-MIT.txt |
| [P2GR/DJM-Rec-for-Android](https://github.com/P2GR/DJM-Rec-for-Android/tree/68f3bb71cff30989ba757ed613a2c6b419b44b0f) | RgbWaveform.kt square-root band envelope and cached translated geometry adapted in source/ui/src/bandGeometry.ts | MIT; full copyright and license in the root THIRD_PARTY_NOTICES.md |
| [chrisle/onelibrary-connect](https://github.com/chrisle/onelibrary-connect) | OneLibrary schema and format-key interoperability reference for the independent Rust reader | MIT; full copyright and license in the root THIRD_PARTY_NOTICES.md; not installed as a library |

## Research and comparison

- **Deep Symmetry Beat Link**, commit ef0aaa1ea949f5c3b1e759973f784c488a12288a: TimeFinder, CdjStatus, WaveformDetail, WaveformPreview, Swing renderers, Util.phraseColor and protocol command references informed independently written transport, waveform and phrase code. Copyright Deep Symmetry LLC, EPL-2.0; no Java classes or library are included.
- **Deep Symmetry crate-digger / DJ Link Ecosystem Analysis / Beat Link Trigger documentation**: ANLZ structures, Pro DJ Link timing, virtual CDJs, loading, sync and stagehand research. The corresponding documentation links are retained below.
- **Mixxx 2.5.6**: filtered waveform renderer reference for per-band maxima, independent heights and drawing layers; its Qt/OpenGL renderer is not bundled.
- **chrisle/alphatheta-connect**: absolute-position protocol and three-band waveform example comparisons; its waveform byte-order/scaling assumptions were not adopted wholesale.
- **fiverecords/SuperTimecodeConverter**: ProDJLinkInput.h comparison in the position-signal audit; credited as a protocol research source, not a bundled dependency.
- **pyrekordbox documentation**: older ANLZ descriptions compared against newer crate-digger and observed data; conflicting PWV7 stride information was not adopted.
- **Pioneer DJ / AlphaTheta / rekordbox**: device manuals, firmware history, OneLibrary information and phrase-analysis documentation used as interoperability references. Trademarks remain their owners' property; OLC is independent and is not endorsed by these companies.
- **Apple developer documentation**: directory access, native application and platform framework guidance.

## Platform and tooling

Application dependencies and full copied notices are enumerated in dependency-inventory.json, CARGO-NOTICES.txt and NPM-NOTICES.txt. SQLCipher, rusqlite and OpenSSL notices are additionally kept in their named files. SQLite is public domain.

System components: [Python](https://www.python.org/), [PyGObject](https://pygobject.gnome.org/), [GTK](https://www.gtk.org/), [WebKitGTK](https://webkitgtk.org/), and Apple [AppKit](https://developer.apple.com/documentation/appkit), [WebKit](https://developer.apple.com/documentation/webkit), [UIKit](https://developer.apple.com/documentation/uikit), [SwiftUI](https://developer.apple.com/documentation/swiftui), [ServiceManagement](https://developer.apple.com/documentation/servicemanagement). These are supplied by the OS or its package manager; they are not vendored here.

Build tools: [Rust/Cargo](https://www.rust-lang.org/), [Swift](https://www.swift.org/), [Xcode](https://developer.apple.com/xcode/), [Node.js](https://nodejs.org/), [npm](https://www.npmjs.com/), [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) and [Zig](https://ziglang.org/). CI uses [actions/checkout](https://github.com/actions/checkout), [actions/setup-node](https://github.com/actions/setup-node), [actions/upload-artifact](https://github.com/actions/upload-artifact), [actions/download-artifact](https://github.com/actions/download-artifact), and the [GitHub CLI](https://cli.github.com/). Build services and tools are not shipped as app code.

## Recorded research URLs

These links preserve the cited evidence without distributing private capture logs, database fixtures or internal research transcripts. Some unpinned upstream links can change over time.

- https://alphatheta.com/en/onelibrary/
- https://alphatheta.com/en/product/player/cdj-1500x/black/
- https://blt-guide.deepsymmetry.org/beat-link-trigger/7.4.1/Integration_BreakBuddy.html
- https://cdn.rekordbox.com/files/20200312172204/rekordbox5.1.0_Phrase_Edit_operation_guide_EN.pdf
- https://cdn.rekordbox.com/files/20210301182245/rekordbox6.5.1_manual_EN.pdf
- https://developer.apple.com/documentation/uikit/providing-access-to-directories
- https://djl-analysis.deepsymmetry.org/djl-analysis/beats.html
- https://djl-analysis.deepsymmetry.org/djl-analysis/loading_tracks.html
- https://djl-analysis.deepsymmetry.org/djl-analysis/stagehand.html
- https://djl-analysis.deepsymmetry.org/djl-analysis/sync.html
- https://djl-analysis.deepsymmetry.org/djl-analysis/vcdj.html
- https://djl-analysis.deepsymmetry.org/rekordbox-export-analysis/anlz.html
- https://downloads.support.alphatheta.com/firmwares/dj-players/CDJ-2000NXS/CDJ-2000NXS-Firmware-Change-History-Ver144-en.pdf
- https://github.com/Deep-Symmetry/beat-link/blob/main/src/main/java/org/deepsymmetry/beatlink/Util.java
- https://github.com/Deep-Symmetry/beat-link/blob/main/src/main/java/org/deepsymmetry/beatlink/VirtualCdj.java
- https://github.com/Deep-Symmetry/beat-link/blob/main/src/main/java/org/deepsymmetry/beatlink/data/TimeFinder.java
- https://github.com/Deep-Symmetry/beat-link/tree/ef0aaa1ea949f5c3b1e759973f784c488a12288a
- https://github.com/Deep-Symmetry/crate-digger/blob/main/src/main/kaitai/rekordbox_anlz.ksy
- https://github.com/P2GR/DJM-Rec-for-Android/tree/68f3bb71cff30989ba757ed613a2c6b419b44b0f
- https://github.com/chrisle/alphatheta-connect
- https://github.com/chrisle/alphatheta-connect/blob/main/docs/ABSOLUTE_POSITION.md
- https://github.com/chrisle/alphatheta-connect/blob/main/examples/3band-waveform-vocal-detection.ts
- https://github.com/chrisle/onelibrary-connect
- https://github.com/chrisle/onelibrary-connect/blob/main/README.md
- https://github.com/fiverecords/SuperTimecodeConverter/blob/main/ProDJLinkInput.h
- https://github.com/mixxxdj/mixxx/blob/2.5.6/src/waveform/renderers/allshader/waveformrendererfiltered.cpp
- https://github.com/nauticsoftware/NauticMixxx
- https://github.com/nauticsoftware/NauticMixxx/tree/v1.0.0
- https://github.com/nauticsoftware/NauticMixxx/tree/v1.0.0/skins/XDJ_RX3_Mixxx
- https://github.com/nsaintot/cdj3k-mods/blob/e011d7342b8fd9557c699a331d3c8c9f705d4e6e/package/deck/mods/wave/codec.c
- https://github.com/nsaintot/cdj3k-mods/tree/e011d7342b8fd9557c699a331d3c8c9f705d4e6e/package/deck
- https://jpn.pioneer/ja/support/manual/manual_pdf.php?m_id=8200
- https://pyrekordbox.readthedocs.io/en/latest/formats/anlz.html
- https://raw.githubusercontent.com/Deep-Symmetry/beat-link/main/src/main/java/org/deepsymmetry/beatlink/VirtualCdj.java
- https://support.alphatheta.com/en-US/articles/48489516530329
- https://support.alphatheta.com/en-us/articles/48525420426009
- https://warehousesound.com/r/pioneerCDJ2000NXSmanual.pdf
- https://warehousesound.com/r/pioneerCDJ3000manual.pdf
- https://www.pioneerdj.com/en-us/support/software/player/cdj-2000nxs2/
- https://www.pioneerdj.com/en/news/2020/cdj-3000-professional-dj-multi-player/
- https://www.pioneerdj.com/ja/news/2016/meet-the-new-cdj-2000nxs2-and-djm-900nxs2/
