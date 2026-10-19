#!/usr/bin/env bash
# Checks standby with the running app: waits until it is in standby, opens
# the Voice Strip microphone for a few seconds as an application would, and
# checks that capture comes back and that the app returns to standby after.
#
# macOS: run from Terminal (it needs the microphone permission) and with
# ffmpeg installed. Linux: needs pw-record. Never against an app you are
# using in a call: it reads its microphone.
#
# Usage: scripts/standby-check.sh [seconds to read, default 8]

set -euo pipefail

cd "$(dirname "$0")/.."
bin=${VOICE_STRIP_BIN:-target/release/voice-strip}
read_s=${1:-8}

now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }
capture() { "$bin" ctl status 2>/dev/null | sed -n 's/.*"capture":"\([a-z]*\)".*/\1/p'; }
wait_for() {
  local want=$1 seconds=$2 i
  for ((i = 0; i < seconds * 10; i++)); do
    [[ $(capture) == "$want" ]] && return 0
    sleep 0.1
  done
  return 1
}
fail() {
  echo "FAIL: $*" >&2
  exit 1
}

[[ -n $(capture) ]] || fail "Voice Strip is not running, or too old to report standby: start it with just start"

echo "waiting for standby (close any application using the Voice Strip microphone)..."
wait_for standby 20 || fail "still capturing after 20 s with nobody reading"
echo "standby"

if [[ $(uname -s) == Darwin ]]; then
  command -v ffmpeg >/dev/null || fail "ffmpeg is needed (brew install ffmpeg)"
  ffmpeg -hide_banner -loglevel error -f avfoundation -i ":Voice Strip" -t "$read_s" -f null - &
else
  command -v pw-record >/dev/null || fail "pw-record is needed (pipewire)"
  timeout "$read_s" pw-record --target voice_strip_mic /dev/null &
fi
reader=$!
start=$(now_ms)
echo "reading the Voice Strip microphone for $read_s s..."
wait_for active 5 || { kill "$reader" 2>/dev/null; fail "capture did not come back within 5 s"; }
echo "active after $(($(now_ms) - start)) ms"
wait "$reader" || true

echo "reader closed, waiting for standby (5 s grace)..."
start=$(now_ms)
wait_for standby 15 || fail "did not return to standby within 15 s"
echo "standby after $(($(now_ms) - start)) ms"
echo "PASS"
