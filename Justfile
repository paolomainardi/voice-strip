# Voice Strip tasks. Run `just help` for an overview.

set shell := ["bash", "-euo", "pipefail", "-c"]
# Recipe arguments reach the shell as "$@", so app recipes forward them to
# the binary unchanged: `just start --input <node> --preset call`.
set positional-arguments

bin := "target/release/voice-strip"
log := "target/voice-strip.log"

# Show this overview
[group('help')]
default: help

# Show the recipes, grouped, with usage notes
[group('help')]
help:
    @just --list --unsorted
    @echo
    @echo "App recipes forward their arguments to voice-strip:"
    @echo "  just start                                   # background, standard view, default mic"
    @echo "  just start --input <node>                    # pick the microphone (see: just devices)"
    @echo "  just start-advanced --preset call --noise strong"
    @echo "  just start --headphones <node>"
    @echo "  just status | just logs | just stop"
    @echo "  voice-strip ctl status | toggle | preset call   # control the running app"
    @echo
    @echo "VOICE_STRIP_INPUT=<node> in the environment is used when --input is not given."
    @echo "Presets: broadcast podcast call stream narration. Noise: off light medium strong."

# ---------------------------------------------------------------- app

# Build the release binary
[group('app')]
build:
    cargo build --release -p app

# Start the app in the background (standard view); arguments go to voice-strip
[group('app')]
[linux]
start *args: build stop
    #!/usr/bin/env bash
    set -euo pipefail
    args=("$@")
    if [[ -n "${VOICE_STRIP_INPUT:-}" && " $* " != *" --input "* ]]; then
        args+=(--input "$VOICE_STRIP_INPUT")
    fi
    setsid -f "{{bin}}" "${args[@]}" >"{{log}}" 2>&1 </dev/null
    sleep 2
    just status

# Start the app in the background (standard view), from Terminal for the microphone permission
[group('app')]
[macos]
start *args: build stop
    #!/usr/bin/env bash
    set -euo pipefail
    args=("$@")
    if [[ -n "${VOICE_STRIP_INPUT:-}" && " $* " != *" --input "* ]]; then
        args+=(--input "$VOICE_STRIP_INPUT")
    fi
    nohup "{{bin}}" "${args[@]}" >"{{log}}" 2>&1 </dev/null &
    sleep 2
    just status

# Start the app in the background in the advanced view; arguments go to voice-strip
[group('app')]
start-advanced *args:
    just start --advanced "$@"

# Run the app in the foreground with any CLI arguments (e.g. `just run --backend sim`)
[group('app')]
run *args:
    cargo run --release -p app -- "$@"

# Run the UI without audio (simulated signal)
[group('app')]
sim *args: build
    "{{bin}}" --backend sim "$@"

# Stop the running app
[group('app')]
stop:
    @pkill -x voice-strip && echo "stopped" || echo "not running"

# Restart the app in the background; arguments go to voice-strip
[group('app')]
restart *args:
    just start "$@"

# Show whether the app runs, its virtual microphone and what it is linked to
[group('app')]
[linux]
status:
    #!/usr/bin/env bash
    if pid=$(pgrep -x voice-strip); then echo "running (pid $pid)"; else echo "not running"; exit 0; fi
    pactl list short sources | grep voice_strip_mic | awk '{print "virtual mic: " $2 " (" $NF ")"}' || true
    echo "capture:"; pw-link -l | grep -A1 '^voice_strip_dsp:input' | sed -n 's/^ *|<- /  /p'
    echo "readers:"; pw-link -l | grep -A10 '^voice_strip_mic:capture' | sed -n 's/^ *|-> /  /p' | sort -u

# Show whether the app and the Voice Strip driver run, and the app's state
[group('app')]
[macos]
status:
    #!/usr/bin/env bash
    if pid=$(pgrep -x voice-strip); then echo "running (pid $pid)"; else echo "not running"; fi
    if pgrep -f "Core Audio Driver \(VoiceStrip.driver\)" >/dev/null; then
        echo "virtual mic: Voice Strip (driver loaded)"
    else
        echo "virtual mic: VoiceStrip.driver not loaded (just macos-driver && just macos-driver-install)"
    fi
    pgrep -x voice-strip >/dev/null && "{{bin}}" ctl status | head -c 200 && echo || true

# Follow the app log
[group('app')]
logs:
    tail -n 50 -f {{log}} | grep --line-buffered -v "Theme parser"

# List microphones and headphones to use with --input and --headphones
[group('app')]
devices: build
    {{bin}} --list-devices 2>/dev/null

# Render the standard and advanced views to docs/screenshots/ (used by the README)
[group('app')]
screenshots: build
    mkdir -p docs/screenshots
    "{{bin}}" --screenshot standard docs/screenshots/standard.png --scale 2
    "{{bin}}" --screenshot advanced docs/screenshots/advanced.png
    @echo "docs/screenshots/standard.png docs/screenshots/advanced.png"

# Link the Omarchy bar widget into ~/.config/omarchy/plugins and enable it
[group('app')]
[linux]
omarchy-plugin:
    mkdir -p ~/.config/omarchy/plugins
    ln -sfn "$PWD/extras/omarchy/io.github.paolomainardi.voicestrip" ~/.config/omarchy/plugins/io.github.paolomainardi.voicestrip
    omarchy-shell shell rescanPlugins
    omarchy plugin enable io.github.paolomainardi.voicestrip

# ---------------------------------------------------------------- macos

# Build target/VoiceStrip.driver, ad-hoc signed (VOICE_STRIP_SIGN_IDENTITY: signed with it)
[group('macos')]
[macos]
macos-driver:
    scripts/macos-driver.sh

# Install target/VoiceStrip.driver and restart coreaudiod (sudo; --uninstall removes it)
[group('macos')]
[macos]
macos-driver-install *args:
    scripts/macos-driver-install.sh "$@"

# Check the installed driver from a client: the shared ring arrives intact at the reported latency (desktop session only)
[group('macos')]
[macos]
test-macos-driver:
    cargo test -p vdev-macos --test driver -- --ignored --nocapture

# Check the backend with the real microphone's callbacks and a test signal (desktop session only)
[group('macos')]
[macos]
test-macos-backend:
    cargo test --release -p vdev-macos --test backend -- --ignored --nocapture --test-threads=1

# ---------------------------------------------------------------- dev

# Format all crates
[group('dev')]
fmt:
    cargo fmt --all

# Format check, clippy with warnings denied, tests: the pre-commit gate
[group('dev')]
check: fetch-model
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

# Run all default tests
[group('dev')]
test: fetch-model
    cargo test --workspace

# Install the git pre-commit hook
[group('dev')]
hooks:
    git config core.hooksPath .githooks

# ---------------------------------------------------------------- measurements

# Download the noise removal model into the user cache (the app asks first, then does the same)
[group('dev')]
fetch-model:
    cargo run --quiet -p denoise-neural --example fetch_model

# Download licensed speech (LibriSpeech, CC BY 4.0) into testdata/
[group('measure')]
fetch-testdata:
    scripts/fetch-testdata.sh

# Real-speech measurements: loudness, tonal balance, dynamics (needs testdata)
[group('measure')]
test-speech:
    cargo test --release -p dsp -p app-core -- --ignored --nocapture --test-threads=1 real_speech processing_reduces narration

# Neural denoiser: realtime, quality and latency on real speech (needs testdata)
[group('measure')]
test-denoise:
    cargo test -p denoise-neural --release -- --nocapture --test-threads=1 --include-ignored

# Timing measurements (CPU budget, neural inference); need an idle machine
[group('measure')]
test-timing:
    cargo test -p dsp --release --test stages cpu_budget -- --ignored --nocapture
    cargo test -p denoise-neural --release --test dfn -- --ignored --nocapture --test-threads=1 inference_time realtime_no_misses

# PipeWire loopback: measured vs reported latency, device switching (private PipeWire server)
[group('measure')]
[linux]
test-pipewire:
    scripts/e2e/run.sh cargo test -p vdev-linux --test loopback -- --ignored --nocapture --test-threads=1

# Record 60 s of your raw microphone into target/voice/ for test-voice (stays local)
[group('measure')]
[linux]
record-voice *source:
    scripts/record-voice.sh {{source}}

# Render a voice recording through every preset, calibrated and not, with a listening page (target/voice/)
[group('measure')]
test-voice recording="target/voice/raw.wav": fetch-model
    cargo run --quiet --release -p app-core --example render_voice -- {{recording}}

# Every preset and noise level on noisy speech through PipeWire (private server; WAVs and report in target/e2e/)
[group('measure')]
[linux]
test-e2e: fetch-model
    [ -f testdata/speech/1272-128104-0003.wav ] || scripts/fetch-testdata.sh
    scripts/e2e/run.sh cargo test -p vdev-linux --test e2e -- --ignored --nocapture
