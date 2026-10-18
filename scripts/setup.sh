#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
# Use an installed compatible Node on PATH. No global tools are installed.
node_bin="${NODE:-node}"
"$node_bin" scripts/check-node.cjs
npm --prefix source/ui ci
cargo fetch --locked
