# Roadmap and known limitations

## Implemented

Offline native three-band analysis, two preview decks, exact-path metadata association, RX3-inspired display, persistent configuration, synthetic tests, pinned Prolink dependency and reproducible local checks. First live monitor: discovery, status, USB/SD metadata and analysis, approximate beat-grid time, and stale-state clearing.

The LIBRARY screen supports read-only playlists, metadata, search, color/rating/My Tag filters, source-scoped saved presets, configurable category filters, header sorting, and protected CDJ1/CDJ2 USB load actions. Local catalog and synthetic parser validation do not certify live hardware behavior.

## Next

1. Validate track changes, USB replacement, network disconnect/reconnect and multiple physical players.
2. Compare estimated position against the CDJ during seeks, pauses, cue jumps and tempo changes; improve sub-beat precision.
3. Add missing MT, pitch range and quantize fields only after verifying their protocol meaning.
4. Add beatgrid/bar markers from actual analysis and refine readability against device screenshots.
5. Package and measure on iPad/Raspberry Pi; macOS remains the development host.

## Deferred

Optional virtual third CDJ serving PC music, host-served track loading and DJM FX if data access is feasible. No implementation is implied by this roadmap.

## Remaining technical debt

- The HTTP/UI contract is maintained manually; add runtime validation or generated types when live transport introduces more states.
- Logical layout remains fixed at 1280×800 and scales to fit; target-device usability is unverified.
- Waveform display tuning and PWV6 normalization are approximate, not firmware-equivalent.
- Browser interaction checks are manual; synthetic tests cover stale-state and asset identity, but hardware transition coverage remains incomplete.
- The vendor subset needs deliberate upstream updates; the bounded My Tags reader is now integrated, with real export/hardware correspondence still to validate.

Housekeeping reduces current debt; it does not establish live correctness or eliminate all future refactoring.
