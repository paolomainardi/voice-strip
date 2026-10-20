#!/usr/bin/env bash
# Builds "target/macos.noindex/Voice Strip.app", the macOS app bundle, and signs it. macOS
# only. The bundle makes the Dock, Cmd+Tab and Spotlight show "Voice Strip"
# with its icon, and gives the microphone permission to the app itself.
#
# The bundle holds the release binary (target/release/voice-strip, built
# first), the Info.plist from extras/macos and an icon made from assets/icon.
# Without VOICE_STRIP_SIGN_IDENTITY it is ad-hoc signed, which needs no Apple
# account. With it, the bundle is signed with that identity (for example
# "Developer ID Application: Name (TEAMID)"), hardened runtime and a
# timestamp, as a release needs.
#
# Usage: scripts/macos-app.sh

set -euo pipefail

if [[ $(uname -s) != Darwin ]]; then
  echo "macos-app: macOS only" >&2
  exit 1
fi

cd "$(dirname "$0")/.."
cargo build --release -p app

BUNDLE="target/macos.noindex/Voice Strip.app"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/app/Cargo.toml | head -n 1)

# Remove the old clone bundle so Spotlight no longer lists it.
rm -rf -- "target/Voice Strip.app"
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources"
sed "s/@VERSION@/$version/" extras/macos/Info.plist > "$BUNDLE/Contents/Info.plist"
cp target/release/voice-strip "$BUNDLE/Contents/MacOS/voice-strip"

iconset=$(mktemp -d)/VoiceStrip.iconset
mkdir -p "$iconset"
png=assets/icon/png/voice-strip
cp "$png-16.png" "$iconset/icon_16x16.png"
cp "$png-32.png" "$iconset/icon_16x16@2x.png"
cp "$png-32.png" "$iconset/icon_32x32.png"
cp "$png-64.png" "$iconset/icon_32x32@2x.png"
cp "$png-128.png" "$iconset/icon_128x128.png"
cp "$png-256.png" "$iconset/icon_128x128@2x.png"
cp "$png-256.png" "$iconset/icon_256x256.png"
cp "$png-512.png" "$iconset/icon_256x256@2x.png"
cp "$png-512.png" "$iconset/icon_512x512.png"
cp "$png-1024.png" "$iconset/icon_512x512@2x.png"
iconutil --convert icns --output "$BUNDLE/Contents/Resources/VoiceStrip.icns" "$iconset"
rm -r "$iconset"

plutil -lint "$BUNDLE/Contents/Info.plist" >/dev/null

identity=${VOICE_STRIP_SIGN_IDENTITY:-}
if [[ -z $identity ]]; then
  codesign --force --sign - "$BUNDLE"
  codesign --verify --strict "$BUNDLE"
  echo "$BUNDLE (ad-hoc signed)"
  exit 0
fi
codesign --force --timestamp --options runtime --sign "$identity" "$BUNDLE"
codesign --verify --strict "$BUNDLE"
echo "$BUNDLE (signed: $identity)"
