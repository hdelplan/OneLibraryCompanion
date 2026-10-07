#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
node_bin="${NODE:-node}"
# Install cargo-zigbuild and Zig explicitly; this script does not change toolchains.
"$node_bin" source/ui/node_modules/typescript/bin/tsc --project source/ui/tsconfig.json --noEmit
(cd source/ui && "$node_bin" node_modules/vite/bin/vite.js build)
cargo zigbuild --release --locked --target aarch64-unknown-linux-gnu.2.36 -p pioneer-companion-host --bin pioneer-companion-host
python3 scripts/package-linux.py --arch arm64 --binary target/aarch64-unknown-linux-gnu/release/pioneer-companion-host
