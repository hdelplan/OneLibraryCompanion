#!/bin/bash
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"
stage="$(mktemp -d "${TMPDIR:-/tmp}/olc-ipad-icon.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
catalog="source/ipad/App/Assets.xcassets"
icons="$catalog/AppIcon.appiconset"
mkdir -p "$icons" target/swift-cache
xcrun swiftc -module-cache-path "$repo_root/target/swift-cache" source/desktop/macos/MakeIcon.swift -o "$stage/make-icon"
"$stage/make-icon" "$icons/icon-1024.png" --ipad
for size in 20 29 40 58 76 80 152 167; do
  sips -z "$size" "$size" "$icons/icon-1024.png" --out "$icons/icon-$size.png" >/dev/null
done
python3 - <<'PY'
import json
from pathlib import Path
catalog = Path('source/ipad/App/Assets.xcassets')
info = {'author': 'xcode', 'version': 1}
images = []
for size, scales in [(20, [1, 2]), (29, [1, 2]), (40, [1, 2]), (76, [1, 2]), (83.5, [2])]:
    for scale in scales:
        images.append({'idiom': 'ipad', 'size': f'{size}x{size}', 'scale': f'{scale}x', 'filename': f'icon-{int(size * scale)}.png'})
images.append({'idiom': 'ios-marketing', 'size': '1024x1024', 'scale': '1x', 'filename': 'icon-1024.png'})
(catalog / 'Contents.json').write_text(json.dumps({'info': info}, indent=2) + '\n')
(catalog / 'AppIcon.appiconset/Contents.json').write_text(json.dumps({'images': images, 'info': info}, indent=2) + '\n')
PY
