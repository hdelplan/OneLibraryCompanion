#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
app="$1"
arch="$2"
stage="$3"
mkdir -p "$stage/payload/Library/PrivilegedHelperTools" "$stage/payload/Library/LaunchDaemons" "$stage/payload/Library/Application Support/OneLibraryCompanion/Networking"
xcrun clang -Wall -Wextra -Werror -O2 -target "$arch-apple-macos13.0" -framework Security -framework CoreFoundation \
  source/desktop/macos/networking/helper.c -o "$stage/payload/Library/PrivilegedHelperTools/org.onelibrarycompanion.networking"
codesign --force --options runtime --sign - "$stage/payload/Library/PrivilegedHelperTools/org.onelibrarycompanion.networking"
cp source/desktop/macos/networking/org.onelibrarycompanion.networking.plist "$stage/payload/Library/LaunchDaemons/"
codesign --display --verbose=4 "$app/Contents/Resources/olc-host" 2> "$stage/host-signature.txt"
sed -n 's/^CDHash=//p' "$stage/host-signature.txt" > "$stage/payload/Library/Application Support/OneLibraryCompanion/Networking/host.cdhash"
python3 - "$stage/payload/Library/Application Support/OneLibraryCompanion/Networking/host.cdhash" <<'PY'
from pathlib import Path
import re,sys
assert re.fullmatch(r'[0-9a-f]{40}\n',Path(sys.argv[1]).read_text()),'Missing host code signature'
PY
chmod 755 "$stage/payload/Library/PrivilegedHelperTools/org.onelibrarycompanion.networking"
chmod 644 "$stage/payload/Library/LaunchDaemons/org.onelibrarycompanion.networking.plist" "$stage/payload/Library/Application Support/OneLibraryCompanion/Networking/host.cdhash"
version=$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$app/Contents/Info.plist")
mkdir -p "$stage/installer"
cp source/desktop/macos/networking/installer/preinstall source/desktop/macos/networking/installer/postinstall "$stage/installer/"
chmod 755 "$stage/installer/"*
pkgbuild --root "$stage/payload" --scripts "$stage/installer" --ownership recommended \
  --identifier org.onelibrarycompanion.networking --version "$version" --install-location / \
  "$app/Contents/Resources/OLC Local USB Support.pkg"
cp source/desktop/macos/networking/uninstall.command "$app/Contents/Resources/Remove Local USB Support.command"
chmod 755 "$app/Contents/Resources/Remove Local USB Support.command"
