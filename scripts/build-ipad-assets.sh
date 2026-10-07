#!/bin/bash
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="$HOME/.cargo/bin:$PATH"
node_bin="${NODE:-$(command -v node || true)}"
if [[ -z "$node_bin" ]] || ! "$node_bin" -e 'const [a,b]=process.versions.node.split(".").map(Number); process.exit((a===20 && b>=19)||(a===22 && b>=12)||a>22 ? 0 : 1)'; then
  node_bin="$HOME/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node"
fi
if [[ ! -x "$node_bin" ]]; then
  echo 'Install Node 22.12+ or set NODE to a compatible executable.' >&2
  exit 1
fi
case "${PLATFORM_NAME:-iphoneos}:${CURRENT_ARCH:-arm64}" in
  iphoneos:*) rust_target=aarch64-apple-ios ;;
  iphonesimulator:x86_64) rust_target=x86_64-apple-ios ;;
  iphonesimulator:*) rust_target=aarch64-apple-ios-sim ;;
  *) echo 'Unsupported Apple platform' >&2; exit 1 ;;
esac
cd "$repo_root"
if ! rustup target list --installed | /usr/bin/grep -qx "$rust_target"; then
  echo "Install the target first: rustup target add $rust_target" >&2
  exit 1
fi
if [[ ! -f source/ui/node_modules/vite/bin/vite.js ]]; then
  echo 'Install UI dependencies first using scripts/setup.sh and a compatible Node.' >&2
  exit 1
fi
xcrun --sdk "${PLATFORM_NAME:-iphoneos}" --show-sdk-path >/dev/null
"$node_bin" source/ui/node_modules/typescript/bin/tsc --project source/ui/tsconfig.json --noEmit
(cd source/ui && "$node_bin" node_modules/vite/bin/vite.js build --mode ipad)
export IPHONEOS_DEPLOYMENT_TARGET=17.6
# Keep device and simulator artifacts separate and preserve desktop build output.
export CARGO_TARGET_DIR="$repo_root/target/ipad"
profile=debug
cargo_args=(build --locked -p pioneer-companion-host --lib --target "$rust_target")
if [[ "${CONFIGURATION:-Debug}" == Release ]]; then
  profile=release
  cargo_args+=(--release)
fi
cargo "${cargo_args[@]}"
if [[ -n "${BUILT_PRODUCTS_DIR:-}" ]]; then
  cp "$CARGO_TARGET_DIR/$rust_target/$profile/libpioneer_companion_host.a" "$BUILT_PRODUCTS_DIR/"
fi
