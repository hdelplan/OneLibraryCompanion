#!/bin/bash
# Read-only readiness check; does not install software or change signing.
set -u
missing=0
echo 'PioneerCompanion iPad build prerequisites'
sw_vers
df -h /
if xcrun --sdk iphoneos --show-sdk-path; then
  xcodebuild -version
else
  echo 'MISSING: full Xcode with iOS platform support selected.'
  missing=1
fi
if command -v rustup >/dev/null 2>&1; then
  targets=$(rustup target list --installed)
  if [[ "$targets" == *aarch64-apple-ios* ]]; then
    echo 'OK: physical iPad Rust target installed.'
  else
    echo 'MISSING: run rustup target add aarch64-apple-ios after Xcode setup.'
    missing=1
  fi
else
  echo 'MISSING: rustup.'
  missing=1
fi
for tool in node npm; do
  if command -v "$tool" >/dev/null 2>&1; then
    "$tool" --version
  else
    echo "MISSING: $tool."
    missing=1
  fi
done
if command -v node >/dev/null 2>&1; then
  if ! node -e 'const [a,b]=process.versions.node.split(".").map(Number); process.exit((a===20 && b>=19)||(a===22 && b>=12)||a>22 ? 0 : 1)'; then
    echo 'MISSING: compatible Node; UI requires 20.19+ (20.x) or 22.12+.'
    missing=1
  fi
fi
echo 'Signing, device trust, Developer Mode and entitlement approval require separate checks.'
exit "$missing"
