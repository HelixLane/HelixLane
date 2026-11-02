#!/usr/bin/env python3
"""Check vendored russh against the published 0.65.0 source fingerprints."""
from pathlib import Path
import hashlib
import json

root = Path(__file__).resolve().parent.parent
expected = json.loads((root / "vendor/russh-upstream-sha256.json").read_text())
package = root / "vendor/russh"
actual = {str(p.relative_to(package)): p for p in package.rglob("*") if p.is_file()}
assert actual.keys() == expected.keys(), "Vendor file set differs from published package"
for name, digest in expected.items():
    content = actual[name].read_bytes()
    if name == "Cargo.toml":
        assert content.count(b'version = "=0.10.0-rc.19"') == 1
        assert content.count(b'version = "=0.8.0-rc.5"') == 1
        content = content.replace(b'version = "=0.10.0-rc.19"', b'version = "=0.10.0-rc.18"')
        content = content.replace(b'version = "=0.8.0-rc.5"', b'version = "=0.8.0-rc.4"')
    if name == "src/keys/format/pkcs8.rs":
        assert content.count(b"pkcs1::RsaPrivateKeyRef::try_from") == 1
        content = content.replace(b"pkcs1::RsaPrivateKeyRef::try_from", b"pkcs1::RsaPrivateKey::try_from")
    assert hashlib.sha256(content).hexdigest() == digest, f"Unexpected change: {name}"
print(f"PASS: {len(expected)} upstream files match; only two manifest pins and one borrowed PKCS#1 type alias differ")
