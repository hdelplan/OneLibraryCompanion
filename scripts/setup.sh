#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
# Use an installed compatible Node on PATH. No global tools are installed.
node -e 'const [major,minor]=process.versions.node.split(".").map(Number);if(!((major===20&&minor>=19)||(major===22&&minor>=12)||major>22)){throw new Error("Use Node 20.19+ (20.x) or 22.12+")}'
npm --prefix source/ui ci
cargo fetch --locked
