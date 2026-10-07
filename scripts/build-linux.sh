#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
node_bin="${NODE:-node}"
arch="$(dpkg --print-architecture)"
case "$arch" in arm64|amd64) ;; *) echo 'Build on 64-bit Debian Bookworm or later (arm64 for Raspberry Pi).' >&2; exit 1 ;; esac
"$node_bin" source/ui/node_modules/typescript/bin/tsc --project source/ui/tsconfig.json --noEmit
(cd source/ui && "$node_bin" node_modules/vite/bin/vite.js build)
cargo build --release --locked -p pioneer-companion-host --bin pioneer-companion-host
python3 scripts/package-linux.py --arch "$arch" --binary target/release/pioneer-companion-host
