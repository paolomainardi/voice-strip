#!/usr/bin/env bash
# Install only Voice Strip's launcher and artwork, never the executable.
set -euo pipefail
[[ $(uname -s) == Linux ]] || { echo 'Desktop entries require Linux' >&2; exit 1; }
executable=
if [[ ${1:-install} == install && $# -ge 2 ]]; then
    executable=$(realpath -- "$2")
    if [[ $executable == *'='* || $executable =~ [[:cntrl:]] ]]; then
        echo 'Desktop executable paths cannot contain = or control characters' >&2
        exit 1
    fi
    # Escape the quoted Exec argument, then the desktop entry string value.
    executable=${executable//\\/\\\\}
    executable=${executable//\"/\\\"}
    executable=${executable//\`/\\\`}
    executable=${executable//\$/\\\$}
    executable=${executable//%/%%}
    executable=${executable//\\/\\\\}
fi
cd "$(dirname "$0")/.."
data_dir=${XDG_DATA_HOME:-"$HOME/.local/share"}
icon_id=io.github.paolomainardi.voicestrip
case ${1:-install} in
    install)
        install -Dm644 "extras/linux/$icon_id.desktop" "$data_dir/applications/$icon_id.desktop"
        if [[ -n $executable ]]; then
            while IFS= read -r line; do
                if [[ $line == Exec=* ]]; then
                    printf 'Exec="%s"\n' "$executable"
                else
                    printf '%s\n' "$line"
                fi
            done < "extras/linux/$icon_id.desktop" > "$data_dir/applications/$icon_id.desktop"
        fi
        for size in 16 32 48 64 128 256 512 1024; do
            install -Dm644 "assets/icon/png/voice-strip-$size.png" \
                "$data_dir/icons/hicolor/${size}x${size}/apps/$icon_id.png"
        done
        install -Dm644 assets/icon/voice-strip.svg "$data_dir/icons/hicolor/scalable/apps/$icon_id.svg"
        ;;
    uninstall)
        rm -f "$data_dir/applications/$icon_id.desktop"
        for size in 16 32 48 64 128 256 512 1024; do
            rm -f "$data_dir/icons/hicolor/${size}x${size}/apps/$icon_id.png"
        done
        rm -f "$data_dir/icons/hicolor/scalable/apps/$icon_id.svg"
        ;;
    *) echo 'Usage: scripts/desktop.sh install [path/to/voice-strip] | uninstall' >&2; exit 2 ;;
esac
if command -v gtk-update-icon-cache >/dev/null; then
    gtk-update-icon-cache --force --ignore-theme-index "$data_dir/icons/hicolor"
fi
