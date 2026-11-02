#!/bin/sh
set -eu
case "${1-}" in /*) binaries=$1 ;; *) echo 'Usage: install.sh /absolute/binaries/directory [app-destination]' >&2; exit 2 ;; esac
app=${2:-${HOME:?}/Applications/HelixLane.app}
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
mkdir -p "$app/Contents/MacOS"
for binary in helixlane helixlane-putty helixlane-winscp; do cp "$binaries/$binary" "$app/Contents/MacOS/$binary"; done
cp "$script_dir/Info.plist" "$app/Contents/Info.plist"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$app"
