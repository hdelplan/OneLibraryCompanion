# Development and Git workflow

## Checks

Run `./scripts/setup.sh` once, then `./scripts/check-app.sh` after changes. Setup installs locked npm dependencies and fetches locked Cargo dependencies. Checks cover application Rust formatting, unit tests, Clippy, Rust build, UI formatting, strict TypeScript including unused declarations, UI tests and production build.

Format with:

```sh
cargo fmt -p pioneer-companion-core -p pioneer-companion-host
npm --prefix source/ui run format
```

Scope Rust formatting to application packages. Do not run broad formatting over the separate Prolink experiments or vendored code. Tests generate synthetic data and do not require a CDJ or private exports. Hardware and real-capture validation is supplementary.

## Repository hygiene

Commit focused changes after checks. Review `git diff --cached --stat` and staged paths before committing. Build outputs, captures, diagnostics, library exports, preservation archives and the local experimental Prolink checkout are ignored. Ignored data is not backed up by Git.

The first commit records the pre-cleanup offline app, including the legacy renderers. The later housekeeping commits make the current checkout reproducible; the initial historical baseline alone still depends on the old local checkout.

Before housekeeping, application sources and local Prolink work were archived under `.local/backups/pre-housekeeping/`, with a separate tracked-change patch. Existing Prolink files were preserved in place. Do not delete that checkout as unused: it contains the earlier protocol/My Tags work needed for live integration.

The reader actually linked by the app is `vendor/prolink`. Its provenance file records the upstream commit and the single workspace-manifest adjustment. Review dependency updates explicitly; do not copy private fixtures into the vendor tree.

## Clean checkout verification

Clone this repository to a temporary directory, run setup and checks there. This verifies that ignored files and the original nested checkout are not required. Network access is needed for uncached public packages. CI runs the same workflow; a local pass is not proof of a hosted CI run.

## Housekeeping validation — 2026-09-29

A fresh local clone passed setup and the complete check script with no captures, library exports or experimental checkout present: 5 Rust tests and 6 UI tests, formatting, TypeScript unused checks, Clippy and both builds. Validated on macOS with Rust 1.98.1 and the bundled compatible Node runtime.

Browser verification loaded the existing private capture through the newly configured host, matched track metadata, scrubbed to 60 seconds and confirmed the RX3 colors, one-third playhead and overview. Health correctly reports offline; malformed analysis returns 400; the removed library route returns 404. The archived Prolink working files were compared byte-for-byte and remain unchanged.

The GitHub workflow is prepared but has not run remotely. No hardware test or publication was performed.
