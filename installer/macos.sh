#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

version=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')
staging=$(mktemp -d "${TMPDIR:-/tmp}/crondesk-package.XXXXXX")
trap 'rm -rf "$staging"' EXIT
app="$staging/CronDesk.app"
mkdir -p "$app/Contents/MacOS" dist

lipo -create target/aarch64-apple-darwin/release/crondesk \
  target/x86_64-apple-darwin/release/crondesk \
  -output "$app/Contents/MacOS/crondesk"
lipo "$app/Contents/MacOS/crondesk" -verify_arch arm64 x86_64

python3 - "$app/Contents/Info.plist" "$version" <<'PY'
import plistlib
import sys
from pathlib import Path
Path(sys.argv[1]).write_bytes(plistlib.dumps({
    "CFBundleDisplayName": "CronDesk",
    "CFBundleExecutable": "crondesk",
    "CFBundleIdentifier": "com.crondesk.desktop",
    "CFBundleName": "CronDesk",
    "CFBundlePackageType": "APPL",
    "CFBundleShortVersionString": sys.argv[2],
    "CFBundleVersion": sys.argv[2],
    "LSMinimumSystemVersion": "11.0",
    "NSHighResolutionCapable": True,
}))
PY

# Ad-hoc signing supports local execution; Developer ID notarization needs Apple credentials.
codesign --force --sign - "$app"
codesign --verify --strict "$app"
ln -s /Applications "$staging/Applications"
hdiutil create -volname CronDesk -srcfolder "$staging" -format UDZO -fs HFS+ \
  -ov "dist/CronDesk-$version-macos-universal.dmg"
hdiutil verify "dist/CronDesk-$version-macos-universal.dmg"
