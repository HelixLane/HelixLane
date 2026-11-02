#!/usr/bin/env python3
"""Emit a desktop entry for an absolute binary; never execute or register it."""
import pathlib
import sys

def desktop_entry(executable):
    path = str(pathlib.Path(executable))
    if not pathlib.Path(path).is_absolute() or any(ord(c) < 32 or ord(c) == 127 for c in path):
        raise ValueError("An absolute executable path without control characters is required")
    # Desktop string escaping happens before Exec argument unquoting.
    escaped = path.replace("\\", "\\\\\\\\").replace('"', '\\\\"').replace("$", "\\\\$").replace("`", "\\\\`").replace("%", "%%")
    return '[Desktop Entry]\nType=Application\nName=HelixLane\nComment=SSH and SFTP workbench\nExec="' + escaped + '" %u\nTerminal=false\nCategories=Network;RemoteAccess;\nMimeType=x-scheme-handler/helixlane;\n'

if __name__ == "__main__":
    sys.stdout.write(desktop_entry(sys.argv[1]))
