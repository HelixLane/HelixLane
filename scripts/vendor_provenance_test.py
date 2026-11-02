#!/usr/bin/env python3
"""Verify portable manifest names and strict raw-byte provenance checks."""
import hashlib
import json
from pathlib import Path, PureWindowsPath
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock

from vendor_provenance import index_vendor_files

ROOT = Path(__file__).resolve().parents[1]


class VendorProvenanceTest(unittest.TestCase):
    def test_windows_names_keep_original_file_objects(self):
        package = Mock()
        file = Mock()
        file.is_file.return_value = True
        file.relative_to.return_value = PureWindowsPath(r"src\keys\format\pkcs8.rs")
        package.rglob.return_value = [file]
        self.assertEqual(index_vendor_files(package), {"src/keys/format/pkcs8.rs": file})
        file.relative_to.assert_called_once_with(package)
        file.read_bytes.assert_not_called()

    def test_verifiers_reject_raw_byte_and_file_set_changes(self):
        for vendor, verifier in [("russh", "verify-russh-vendor.py"),
                                 ("gpui-shell", "verify-shell-vendor.py")]:
            with self.subTest(vendor=vendor), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                scripts = root / "scripts"
                scripts.mkdir()
                for name in [verifier, "vendor_provenance.py"]:
                    shutil.copyfile(ROOT / "scripts" / name, scripts / name)
                package = root / "vendor" / vendor
                package.mkdir(parents=True)
                source = package / "source.rs"
                original = b"// upstream\n// already CRLF\r\n"
                source.write_bytes(original)
                manifest = root / "vendor" / f"{vendor}-upstream-sha256.json"
                manifest.write_text(json.dumps({"source.rs": hashlib.sha256(original).hexdigest()}))

                def verify():
                    return subprocess.run([sys.executable, "-B", str(scripts / verifier)],
                                          capture_output=True, text=True)

                result = verify()
                self.assertEqual(result.returncode, 0, result.stderr)
                source.write_bytes(original.replace(b"upstream\n", b"upstream\r\n"))
                result = verify()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Unexpected change: source.rs", result.stderr)
                source.write_bytes(original)
                (package / "extra.rs").write_bytes(b"unexpected")
                result = verify()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Vendor file set differs", result.stderr)

    def test_git_autocrlf_preserves_vendor_bytes_with_attributes(self):
        for protected in [False, True]:
            with self.subTest(protected=protected), tempfile.TemporaryDirectory() as temp:
                repo = Path(temp) / "repo"
                repo.mkdir()
                checkout = Path(temp) / "checkout"
                checkout.mkdir()
                source = repo / "vendor/russh/README.md"
                source.parent.mkdir(parents=True)
                original = (ROOT / "vendor/russh/README.md").read_bytes()
                source.write_bytes(original)
                crlf = repo / "vendor/upstream-crlf.txt"
                crlf.write_bytes(b"upstream CRLF\r\n")
                mixed = repo / "vendor/mixed.bin"
                mixed.write_bytes(b"LF\nCRLF\r\n\x00exact\xff")
                if protected:
                    shutil.copyfile(ROOT / ".gitattributes", repo / ".gitattributes")
                def git(*args):
                    subprocess.run(["git", *args], cwd=repo, check=True,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                git("init", "-q")
                git("-c", "core.autocrlf=false", "add", ".")
                git("-c", "core.autocrlf=true", "checkout-index", "--all",
                    f"--prefix={checkout.as_posix()}/")
                actual = (checkout / "vendor/russh/README.md").read_bytes()
                if protected:
                    self.assertEqual(actual, original)
                    self.assertEqual((checkout / "vendor/upstream-crlf.txt").read_bytes(), crlf.read_bytes())
                    self.assertEqual((checkout / "vendor/mixed.bin").read_bytes(), mixed.read_bytes())
                else:
                    self.assertNotEqual(actual, original)
                    self.assertEqual(actual, original.replace(b"\n", b"\r\n"))


if __name__ == "__main__":
    unittest.main()
