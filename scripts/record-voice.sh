#!/usr/bin/env bash
# Records 60 s of a raw microphone into target/voice/raw.wav for
# `just test-voice`. The file stays on this machine: target/ is not
# committed. Linux (PipeWire).
#
# Usage: scripts/record-voice.sh [source node name]   (see `just devices`)

set -euo pipefail

cd "$(dirname "$0")/.."
mkdir -p target/voice
out=target/voice/raw.wav
target=()
if [[ -n ${1:-} ]]; then
  target=(--target "$1")
fi
cat <<'TXT'
Recording 60 s. Stay silent for the first 5 s (the room, for calibration),
then read naturally: some plosives (P, B), some sibilants (S), a quiet line,
a loud line, a 3 s pause, and a last line. Stay silent until it stops.
TXT
timeout 60 pw-record "${target[@]}" --rate 48000 --channels 1 --format f32 "$out" || true
echo "Recorded $out. Next: just test-voice"
