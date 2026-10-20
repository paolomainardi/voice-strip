#!/usr/bin/env bash
# Regenerate the committed app icons. Requires librsvg's rsvg-convert.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p assets/icon/png
for size in 16 32 48 64 128 256 512 1024; do
    if (( size <= 32 )); then
        # At small sizes, keep the bars crisp and omit screws, glow and shadow.
        style='.detail { display: none; } path { shape-rendering: crispEdges; }'
        if (( size == 16 )); then
            style+=' .full-mark { display: none; } .small-mark { display: inline; }'
        fi
        printf '%s' "$style" |
            rsvg-convert --stylesheet /dev/stdin -w "$size" -h "$size" assets/icon/voice-strip.svg \
                -o "assets/icon/png/voice-strip-${size}.png"
    else
        rsvg-convert -w "$size" -h "$size" assets/icon/voice-strip.svg \
            -o "assets/icon/png/voice-strip-${size}.png"
    fi
done
