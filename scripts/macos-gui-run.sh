#!/usr/bin/env bash
# Runs a command in a Terminal window of the desktop session, waits for it,
# prints its output and exits with its status. macOS only.
#
# Processes started over SSH never get the microphone permission (macOS
# hands them silence), while Terminal keeps the one the user granted it, so
# captures run through this script from an SSH session.
#
# Usage: scripts/macos-gui-run.sh <command> [args...]

set -euo pipefail

if [[ $# -eq 0 ]]; then
  echo "usage: $(basename "$0") <command> [args...]" >&2
  exit 2
fi

cd "$(dirname "$0")/.."
dir="$PWD/target/gui-run"
mkdir -p "$dir"
run="$dir/$(date +%s)-$$"

{
  echo '#!/bin/bash'
  printf 'cd %q\n' "$PWD"
  printf 'export PATH=%q\n' "$PATH"
  printf '%q ' "$@"
  printf '>%q 2>&1\n' "$run.log"
  printf 'echo $? >%q\n' "$run.rc"
  echo 'exit'
} >"$run.command"
chmod +x "$run.command"
open -a Terminal "$run.command"

for _ in $(seq 1 600); do
  [[ -f $run.rc ]] && break
  sleep 1
done
if [[ ! -f $run.rc ]]; then
  echo "macos-gui-run: no result after 600 s, see $run.log" >&2
  exit 1
fi
cat "$run.log"
exit "$(cat "$run.rc")"
