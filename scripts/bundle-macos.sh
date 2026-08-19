#!/usr/bin/env bash
# Wrap the built `sluice` binary in a minimal macOS .app bundle (dev convenience;
# the release pipeline will add icon, signing and notarization — 05 §9.4).
# Usage: scripts/bundle-macos.sh [debug|release]  ->  target/<profile>/Sluice.app
set -euo pipefail
PROFILE="${1:-debug}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/$PROFILE/sluice"
APP="$ROOT/target/$PROFILE/Sluice.app"
VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')"
[ -x "$BIN" ] || { echo "build first: cargo build ${PROFILE/debug/} -p sluice" >&2; exit 1; }
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/sluice"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Sluice</string>
  <key>CFBundleDisplayName</key><string>Sluice</string>
  <key>CFBundleIdentifier</key><string>ai.kverona.sluice</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleExecutable</key><string>sluice</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
</dict></plist>
PLIST
echo "$APP"
