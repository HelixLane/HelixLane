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
    if name == "README.md":
        lines = content.splitlines(keepends=True)
        assert lines[2] == b'[![Crate](https://img.shields.io/crates/v/russh.svg)](https://crates.io/crates/russh)\n'
        lines[2] = b'[![Crate](https://img.shields.io/crates/v/russh.svg)](https://crates.io/crates/russh) \n'
        assert lines[131] == b'Russh is built to withstand malicious/misbehaving peers.\n'
        lines[131] = b'Russh is built to withstand malicious/misbehaving peers. \n'
        assert lines[188] == b'Russh began as a fork of [Thrussh](https://nest.pijul.com/pijul/thrussh) by Pierre-\xc3\x89tienne Meunier, originally extended to provide the SSH backend for [Warpgate](https://github.com/warp-tech/warpgate).\n'
        lines[188] = b'Russh began as a fork of [Thrussh](https://nest.pijul.com/pijul/thrussh) by Pierre-\xc3\x89tienne Meunier, originally extended to provide the SSH backend for [Warpgate](https://github.com/warp-tech/warpgate). \n'
        content = b"".join(lines)
    assert hashlib.sha256(content).hexdigest() == digest, f"Unexpected change: {name}"
print(f"PASS: {len(expected)} upstream files match; only two manifest pins, one borrowed PKCS#1 type alias and three exact README whitespace normalizations differ")
