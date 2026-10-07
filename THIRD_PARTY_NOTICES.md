# OneLibraryCompanion — third-party acknowledgments

The app links to pinned Prolink networking, protocol and Rekordbox reader crates under `vendor/prolink` (GPL-3.0-only). See `vendor/prolink/PROVENANCE.md` in the source repository for the pinned upstream commit and all local modifications; a copy is shipped in `third-party-licenses/Prolink-PROVENANCE.md`. Original license notices are included.

The UI geometry and summary rendering convention are adapted from NauticMixxx v1.0.0, commit `4a72826059d2b0041057c94e9d65bc82fd50c9fa`, copyright its listed contributors. Its repository includes a GPL v3 license; its upstream notice identifies the Mixxx 2.5.6 base as GPL-2.0-or-later and the original ntamas94/pioneered-by-ntamas skin as GPL v3. The original license and notices are reproduced in `third-party-licenses/NauticMixxx-LICENSE.txt` and `third-party-licenses/NauticMixxx-THIRD-PARTY-NOTICES.md`. OLC is distributed under GPL-3.0-only. See `third-party-licenses/SOURCE-CREDITS.md` for the exact source mapping. No logo, screenshot, skin artwork or proprietary font has been copied.

The band-set rendering algorithm and palette are based on nsaintot/cdj3k-mods, available under MIT OR Apache-2.0. Its MIT notice is reproduced in `third-party-licenses/cdj3k-mods-LICENSE-MIT.txt`.

Format research: Deep Symmetry crate-digger / DJ Link Ecosystem Analysis. Source links, including the older pyrekordbox documentation used for comparison, are recorded in `third-party-licenses/SOURCE-CREDITS.md`.

React, Vite, TypeScript, Axum, Tokio and other pinned dependencies are credited, with their license and notice texts reproduced, in `third-party-licenses/NPM-NOTICES.txt` and `third-party-licenses/CARGO-NOTICES.txt`. The machine-readable `third-party-licenses/dependency-inventory.json` records versions, declared licenses and source repositories. OneLibraryCompanion is an independent project; product names identify compatibility targets, not endorsement.

Additional reference research: Deep Symmetry Beat Link (`ef0aaa1ea949f5c3b1e759973f784c488a12288a`, copyright Deep Symmetry LLC, EPL-2.0). Numerical PWV6/PWV7 scaling, protocol facts and Rekordbox phrase-palette values informed independently written Rust/TypeScript implementations; no Java library or source classes are included. See `third-party-licenses/SOURCE-CREDITS.md` for exact source links and the distinction between research references and adapted code.

## DJM Rec for Android waveform method

`source/ui/src/bandGeometry.ts` adapts square-root band envelopes and cached translated geometry from P2GR/DJM-Rec-for-Android, `RgbWaveform.kt`, commit `68f3bb71cff30989ba757ed613a2c6b419b44b0f`. The implementation uses full-track ANLZ data and Pro DJ Link position rather than a live audio history.

Source: https://github.com/P2GR/DJM-Rec-for-Android

MIT License

Copyright (c) 2026 P2GR

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## jsPDF

The SET HISTORY PDF exporter uses jsPDF (MIT), installed through npm. Its full copyright and license text, and those of its installed dependencies, are included in `third-party-licenses/NPM-NOTICES.txt`. The exporter renders Unicode text with the browser canvas and embeds paginated images; it does not send track data to an external service.
## OneLibrary interoperability reference

Schema and format-key information from https://github.com/chrisle/onelibrary-connect.

MIT License

Copyright (c) 2025 Chris Le

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## OneLibrary database runtime

The shared host uses rusqlite (MIT) with bundled SQLCipher (Zetetic BSD-style
license) and vendored OpenSSL (Apache-2.0). SQLite itself is public domain.
Pinned dependency versions are in Cargo.lock; upstream notices are retained
with the dependency sources and in third-party-licenses/.

## Native platform components and build tools

The macOS shell uses Apple AppKit, WebKit and ServiceManagement. These frameworks are supplied by the operating system. The Linux desktop uses Python, PyGObject, GTK 3 and WebKitGTK 4.1, installed by Raspberry Pi OS/Debian packages with their own notices. Rust/Cargo, Swift/Xcode, Node.js/npm, cargo-zigbuild and Zig are build tools, not copied application source. See `third-party-licenses/SOURCE-CREDITS.md` for links and scope.

The license inventory includes all resolved Cargo packages (including build and target-specific dependencies) and the installed npm production and development packages. Uninstalled optional npm build executables for other platforms are excluded; they are not bundled in the app. Native OpenSSL and SQLCipher notices are included separately. Regenerate dependency notices after lockfile updates with `python3 scripts/collect-third-party-notices.py`.
