#!/usr/bin/env bash
# Runs a command against a private PipeWire server with WirePlumber, so
# PipeWire tests never touch the desktop's audio, hardware or a running
# Voice Strip. The server, its sockets and WirePlumber's config and state
# live in a temporary folder that is removed afterwards. Linux only.
#
# Usage: scripts/e2e/run.sh <command> [args...]

set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)

for tool in pipewire wireplumber pw-dump; do
  command -v "$tool" >/dev/null || { echo "e2e: $tool is not installed" >&2; exit 1; }
done

# Short base path: the socket path must fit in sun_path.
tmp=$(mktemp -d "${XDG_RUNTIME_DIR:-/tmp}/vs-e2e.XXXXXX")
pids=()
cleanup() {
  for pid in "${pids[@]}"; do
    kill "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
  rm -rf "$tmp"
}
trap cleanup EXIT INT TERM

export PIPEWIRE_RUNTIME_DIR=$tmp
mkdir -p "$tmp/config/wireplumber/wireplumber.conf.d" "$tmp/state"
cp "$here/wireplumber.conf" "$tmp/config/wireplumber/wireplumber.conf.d/e2e.conf"

pipewire -c "$here/pipewire.conf" >"$tmp/pipewire.log" 2>&1 &
pids+=($!)
for _ in $(seq 50); do
  [[ -S $tmp/pipewire-0 ]] && break
  sleep 0.1
done
[[ -S $tmp/pipewire-0 ]] || { cat "$tmp/pipewire.log" >&2; exit 1; }

env -u DBUS_SESSION_BUS_ADDRESS XDG_CONFIG_HOME="$tmp/config" XDG_STATE_HOME="$tmp/state" \
  wireplumber >"$tmp/wireplumber.log" 2>&1 &
pids+=($!)
# Ready once WirePlumber shows up as a client of the server.
wireplumber_ready() {
  local dump
  dump=$(timeout 2 pw-dump 2>/dev/null) || return 1
  [[ $dump == *'"application.name": "WirePlumber"'* ]]
}
for _ in $(seq 100); do
  wireplumber_ready && break
  sleep 0.1
done
wireplumber_ready || { cat "$tmp/wireplumber.log" >&2; exit 1; }

"$@"
