#!/usr/bin/env python3
"""Cross-platform archive-name regression; runs without compiled binaries."""
import hashlib
import io
import pathlib
import unittest
import zipfile

from package import archive_name


class PackageNamesTest(unittest.TestCase):
    def test_windows_relative_members_match_manifest_and_zip(self):
        root = pathlib.PureWindowsPath(r"C:\build\HelixLane")
        members = {
            r"vendor\russh\LICENSE": "vendor/russh/LICENSE",
            r"docs\launch.md": "docs/launch.md",
            r"packaging\windows\register.ps1": "packaging/windows/register.ps1",
        }
        payload = {}
        for relative, expected in members.items():
            with self.subTest(relative=relative):
                name = archive_name(root / relative, root)
                self.assertEqual(name, expected)
                payload[name] = relative.encode()
        manifest = "".join(
            f"{hashlib.sha256(data).hexdigest()}  {name}\n"
            for name, data in sorted(payload.items())
        )
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            for name, data in payload.items():
                archive.writestr(zipfile.ZipInfo(name), data)
            archive.writestr("SHA256SUMS", manifest)
        with zipfile.ZipFile(buffer) as archive:
            self.assertEqual(set(archive.namelist()), {*members.values(), "SHA256SUMS"})
            for line in archive.read("SHA256SUMS").decode().splitlines():
                digest, name = line.split("  ", 1)
                self.assertEqual(hashlib.sha256(archive.read(name)).hexdigest(), digest)


if __name__ == "__main__":
    unittest.main()
