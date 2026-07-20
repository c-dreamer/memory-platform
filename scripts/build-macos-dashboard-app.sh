#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST="$ROOT/dist"
APP="$DIST/Memory Platform.app"
DMG="$DIST/Memory-Platform-unsigned.dmg"

command -v swiftc >/dev/null || { echo "Swift is required to build the macOS app" >&2; exit 78; }
rm -rf "$APP" "$DMG"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$ROOT/macos-app/Info.plist" "$APP/Contents/Info.plist"
swiftc -O -parse-as-library -framework SwiftUI -framework WebKit -framework AppKit \
  "$ROOT/macos-app/MemoryPlatformApp.swift" \
  -o "$APP/Contents/MacOS/MemoryPlatform"
chmod 755 "$APP/Contents/MacOS/MemoryPlatform"
hdiutil create -quiet -volname "Memory Platform" -srcfolder "$APP" -ov -format UDZO "$DMG"
echo "Built unsigned app: $APP"
echo "Built unsigned DMG: $DMG"
