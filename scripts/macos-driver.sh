#!/usr/bin/env bash
# Builds target/VoiceStrip.driver, the macOS virtual microphone bundle, and
# signs it. macOS only.
#
# The bundle's Info.plist gets the release version and
# VoiceStripDriverRevision, a hash of what the driver is built from (its
# sources, Info.plist, its resolved dependencies and the manifests without
# their version lines), so
# `voice-strip driver install --if-needed` replaces an installed driver only
# when its code changed.
#
# Without VOICE_STRIP_SIGN_IDENTITY the bundle is ad-hoc signed, which
# coreaudiod loads on a development machine. With it, the bundle is signed
# with that identity (for example "Developer ID Application: Name (TEAMID)"),
# hardened runtime and a timestamp, as a release needs.
#
# Signing over SSH: the login keychain is locked in an SSH session, so keep
# the identity in its own keychain and set VOICE_STRIP_KEYCHAIN (its path)
# and VOICE_STRIP_KEYCHAIN_PASSWORD. The script unlocks it and, while
# codesign runs, makes it the only keychain in the search list: a locked
# login keychain there makes codesign fail even with --keychain. The list is
# restored right after, also on error.
#
# Usage: scripts/macos-driver.sh

set -euo pipefail

BUNDLE=target/VoiceStrip.driver

if [[ $(uname -s) != Darwin ]]; then
  echo "macos-driver: macOS only" >&2
  exit 1
fi

cd "$(dirname "$0")/.."
cargo build --release -p vdev-macos-driver

rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS"
cp crates/vdev-macos-driver/Info.plist "$BUNDLE/Contents/Info.plist"
cp target/release/libvdev_macos_driver.dylib "$BUNDLE/Contents/MacOS/VoiceStrip"

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' crates/vdev-macos-driver/Cargo.toml | head -n 1)
revision=$(
  {
    find crates/vdev-macos-driver/src -type f -name '*.rs' | LC_ALL=C sort | xargs cat
    cat crates/vdev-macos-driver/Info.plist
    grep -v '^version = ' crates/vdev-macos-driver/Cargo.toml
    grep -v '^version = ' Cargo.toml
    # Dependencies as Cargo.lock resolves them, without the driver's own line.
    cargo tree --locked -p vdev-macos-driver -e normal,build --prefix none | sed 1d | LC_ALL=C sort -u
  } | shasum -a 256 | cut -c1-16
)
plist="$BUNDLE/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$plist"
/usr/libexec/PlistBuddy -c "Add :VoiceStripDriverRevision string $revision" "$plist"

identity=${VOICE_STRIP_SIGN_IDENTITY:-}
if [[ -z $identity ]]; then
  codesign --force --sign - "$BUNDLE"
  echo "$BUNDLE (ad-hoc signed)"
  exit 0
fi

keychain=()
if [[ -n ${VOICE_STRIP_KEYCHAIN:-} ]]; then
  kc=$VOICE_STRIP_KEYCHAIN
  security unlock-keychain -p "${VOICE_STRIP_KEYCHAIN_PASSWORD:?set VOICE_STRIP_KEYCHAIN_PASSWORD}" "$kc"
  saved=()
  while IFS= read -r k; do
    k=${k#"${k%%[![:space:]]*}"}
    saved+=("${k//\"/}")
  done < <(security list-keychains -d user)
  trap 'security list-keychains -d user -s "${saved[@]}"' EXIT HUP INT TERM
  security list-keychains -d user -s "$kc"
  keychain=(--keychain "$kc")
fi
codesign --force --timestamp --options runtime "${keychain[@]}" --sign "$identity" "$BUNDLE"
codesign --verify --strict "$BUNDLE"
echo "$BUNDLE (signed: $identity)"
