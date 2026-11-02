#!/bin/sh
set -eu
case "${1-}" in /*) executable=$1 ;; *) echo 'Usage: install.sh /absolute/path/to/helixlane' >&2; exit 2 ;; esac
[ -x "$executable" ] || { echo 'Executable unavailable' >&2; exit 2; }
case "${XDG_DATA_HOME-}" in /*) data_root=$XDG_DATA_HOME ;; *) data_root=${HOME:?}/.local/share ;; esac
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
mkdir -p "$data_root/applications"
python3 "$script_dir/generate_desktop.py" "$executable" > "$data_root/applications/helixlane.desktop"
if command -v update-desktop-database >/dev/null 2>&1; then update-desktop-database "$data_root/applications"; fi
xdg-mime default helixlane.desktop x-scheme-handler/helixlane
