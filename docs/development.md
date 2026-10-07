# Development and Git workflow

## Checks

Run `./scripts/setup.sh` once, then `./scripts/check-app.sh` after changes. Setup installs locked npm dependencies and fetches locked Cargo dependencies. Checks cover application Rust formatting, unit tests, Clippy, Rust build, UI formatting, strict TypeScript including unused declarations, UI tests and production build.

Format with:

```sh
cargo fmt -p pioneer-companion-core -p pioneer-companion-host
npm --prefix source/ui run format
```

Scope Rust formatting to application packages. Do not run broad formatting over vendored code. Tests generate synthetic data and do not require a CDJ or private exports. Hardware and real-capture validation is supplementary.

## Repository hygiene

Commit focused changes after checks. Review staged paths before committing. Keep build outputs, captures, library exports and credentials out of Git. The linked reader is `vendor/prolink`; its provenance document records upstream identity and local modifications. Preserve license notices when updating dependencies.

## Clean checkout verification

Clone the repository into a temporary directory and run setup and checks. Network access is needed for uncached public dependencies. See [build validation](build-validation.md) for distribution status.
