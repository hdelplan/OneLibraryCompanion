#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
repo_root="$PWD"
arch="${1:-$(uname -m)}"
case "$arch" in
  arm64) rust_target=aarch64-apple-darwin ;;
  x86_64) rust_target=x86_64-apple-darwin ;;
  *) echo 'Usage: scripts/build-macos.sh [arm64|x86_64]' >&2; exit 1 ;;
esac
node_bin="${NODE:-node}"
if ! rustup target list --installed | grep -qx "$rust_target"; then
  echo "Install the Rust target first: rustup target add $rust_target" >&2; exit 1
fi
"$node_bin" source/ui/node_modules/typescript/bin/tsc --project source/ui/tsconfig.json --noEmit
(cd source/ui && "$node_bin" node_modules/vite/bin/vite.js build)
export MACOSX_DEPLOYMENT_TARGET=13.0
cargo build --release --locked -p pioneer-companion-host --bin pioneer-companion-host --target "$rust_target"
stage="$repo_root/builds/macos-$arch"
# Replace generated artifacts only; never touch development sources or user data.
mkdir -p "$stage"
app="$stage/OneLibraryCompanion.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
xcrun swiftc -parse-as-library -O -target "$arch-apple-macos13.0" -module-cache-path "$repo_root/target/swift-cache" \
  -framework Cocoa -framework WebKit -framework ServiceManagement source/desktop/macos/OLC.swift -o "$app/Contents/MacOS/OLC"
cp source/desktop/macos/Info.plist "$app/Contents/Info.plist"
cp "target/$rust_target/release/pioneer-companion-host" "$app/Contents/Resources/olc-host"
# ditto --norsrc copies a fresh Vite directory; staging is architecture-specific.
python3 - "$app" <<'PY'
from pathlib import Path
import shutil,sys
resources=Path(sys.argv[1])/'Contents/Resources'
if (resources/'dist').exists(): shutil.rmtree(resources/'dist')
shutil.copytree('source/ui/dist',resources/'dist')
for name in ['LICENSE','THIRD_PARTY_NOTICES.md']: shutil.copy2(name,resources/name)
shutil.copytree('third-party-licenses',resources/'third-party-licenses',dirs_exist_ok=True)
PY
xcrun swiftc -module-cache-path "$repo_root/target/swift-cache" source/desktop/macos/MakeIcon.swift -o "$stage/make-icon"
"$stage/make-icon" "$stage/icon.png"
icons="$stage/OLC.iconset"
mkdir -p "$icons"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$stage/icon.png" --out "$icons/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" "$stage/icon.png" --out "$icons/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$icons" -o "$app/Contents/Resources/OLC.icns"
codesign --force --options runtime --sign - "$app/Contents/Resources/olc-host"
./scripts/package-mac-networking.sh "$app" "$arch" "$stage/networking"
codesign --force --sign - "$app"
codesign --verify --deep --strict "$app"
version=$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$app/Contents/Info.plist")
mkdir -p builds/releases
archive="builds/releases/OneLibraryCompanion-$version-macos-$arch.zip"
ditto -c -k --sequesterRsrc --keepParent "$app" "$archive"
if [[ "${OLC_SKIP_DMG:-0}" != 1 ]]; then
  image_dir="$stage/dmg"
  mkdir -p "$image_dir"
  ditto "$app" "$image_dir/OneLibraryCompanion.app"
  ln -sfn /Applications "$image_dir/Applications"
  hdiutil create -ov -format UDZO -volname OneLibraryCompanion -srcfolder "$image_dir" "builds/releases/OneLibraryCompanion-$version-macos-$arch.dmg"
fi
shasum -a 256 "$archive" > "$archive.sha256"
if [[ -f "builds/releases/OneLibraryCompanion-$version-macos-$arch.dmg" ]]; then
  shasum -a 256 "builds/releases/OneLibraryCompanion-$version-macos-$arch.dmg" > "builds/releases/OneLibraryCompanion-$version-macos-$arch.dmg.sha256"
fi
echo "Built $app"
