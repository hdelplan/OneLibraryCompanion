#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
NODE="${NODE:-node}"
# Format application packages only, never local or vendored experiments.
cargo fmt -p pioneer-companion-core -p pioneer-companion-host -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --no-deps --locked -- -D warnings
cargo build --workspace --locked
(
  cd source/ui
  "$NODE" node_modules/prettier/bin/prettier.cjs --check src vite.config.ts tsconfig.json package.json
  "$NODE" node_modules/typescript/bin/tsc --noEmit --noUnusedLocals --noUnusedParameters
  "$NODE" --import tsx --test src/*.test.ts
  "$NODE" node_modules/vite/bin/vite.js build
)
