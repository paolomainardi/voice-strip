#!/usr/bin/env bash
# Installs, or removes, the Voice Strip virtual microphone driver and
# restarts coreaudiod so it loads. Audio on the Mac stops for a moment.
#
# Before installing, the script checks the bundle's signature. With
# VOICE_STRIP_TEAM set, it also requires that team ID, so only a bundle
# signed with that identity lands in the HAL folder.
#
# Usage: scripts/macos-driver-install.sh [path/to/VoiceStrip.driver]
#        scripts/macos-driver-install.sh --uninstall

set -euo pipefail

TEAM=${VOICE_STRIP_TEAM:-}
HAL=/Library/Audio/Plug-Ins/HAL
INSTALLED="$HAL/VoiceStrip.driver"
DEFAULT="$(cd "$(dirname "$0")/.." && pwd)/target/VoiceStrip.driver"

helper_pid() {
  pgrep -f "Core Audio Driver \(VoiceStrip.driver\)" || true
}

# Stops a running Voice Strip first: an app attached to the device while
# coreaudiod restarts can get stuck reconnecting and keep coreaudiod busy.
stop_app() {
  pgrep -x voice-strip >/dev/null || return 0
  echo "stopping Voice Strip during the driver change; start it again afterwards"
  pkill -x voice-strip || true
  for _ in $(seq 1 50); do
    pgrep -x voice-strip >/dev/null || return 0
    sleep 0.1
  done
  echo "Voice Strip did not stop; quit it and retry" >&2
  exit 1
}

# Waits until Core Audio lists the Voice Strip device again, so nothing
# starts while every client is still reconnecting.
settle() {
  for _ in $(seq 1 30); do
    if system_profiler SPAudioDataType 2>/dev/null | grep -q "Voice Strip:"; then
      return
    fi
    sleep 1
  done
  echo "Core Audio does not list Voice Strip yet; wait a moment before starting the app" >&2
}

# Restarts coreaudiod and waits for a new driver process, not the old one
# still exiting.
restart() {
  local old
  old=$(helper_pid)
  sudo killall coreaudiod
  for _ in $(seq 1 20); do
    local now
    now=$(helper_pid)
    if [[ -n $now && $now != "$old" ]]; then
      echo "VoiceStrip.driver loaded (pid $now)"
      settle
      return
    fi
    sleep 0.5
  done
  echo "VoiceStrip.driver did not load, check: /usr/bin/log show --last 1m --predicate 'eventMessage CONTAINS \"VoiceStrip\"'" >&2
  exit 1
}

if [[ ${1:-} == --uninstall ]]; then
  if [[ -d $INSTALLED ]]; then
    stop_app
    sudo rm -r "$INSTALLED"
    sudo killall coreaudiod
    echo "removed $INSTALLED"
  else
    echo "not installed"
  fi
  exit 0
fi

bundle=${1:-$DEFAULT}
bundle=${bundle%/}
if [[ ! -d $bundle || $(basename "$bundle") != VoiceStrip.driver ]]; then
  echo "usage: $(basename "$0") [path/to/VoiceStrip.driver] | --uninstall" >&2
  exit 2
fi

codesign --verify --strict "$bundle"
team=$(codesign -dvv "$bundle" 2>&1 | sed -n 's/^TeamIdentifier=//p')
if [[ -n $TEAM && $team != "$TEAM" ]]; then
  echo "$bundle is not signed by team $TEAM (found: ${team:-none})" >&2
  exit 1
fi

stop_app
if [[ -d $INSTALLED ]]; then
  sudo rm -r "$INSTALLED"
fi
sudo cp -R "$bundle" "$HAL/"
echo "installed $INSTALLED"
restart
