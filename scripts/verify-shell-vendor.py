#!/usr/bin/env python3
"""Verify the published shell source plus a per-policy environment opt-out."""
from pathlib import Path
import hashlib
import json

root = Path(__file__).resolve().parent.parent
expected = json.loads((root / "vendor/gpui-shell-upstream-sha256.json").read_text())
package = root / "vendor/gpui-shell"
actual = {str(p.relative_to(package)): p for p in package.rglob("*") if p.is_file()}
assert actual.keys() == expected.keys(), "Vendor file set differs from published package"
insertions = [
    b"    deny_default_env: bool,\n",
    b"    /// Disables implicit environment access for an embedded plugin.\n"
    b"    /// Explicit names granted with `env_variables` remain visible.\n"
    b"    pub fn without_default_env(mut self) -> Self {\n"
    b"        self.deny_default_env = true;\n"
    b"        self\n"
    b"    }\n\n",
    b"            .filter(|_| !self.deny_default_env)\n",
]
for name, digest in expected.items():
    content = actual[name].read_bytes()
    if name == "src/capability.rs":
        for insertion in insertions:
            assert content.count(insertion) == 1, f"Unexpected opt-out patch shape: {insertion!r}"
            content = content.replace(insertion, b"", 1)
    assert hashlib.sha256(content).hexdigest() == digest, f"Unexpected change: {name}"
print(f"PASS: {len(expected)} upstream files match; only the per-policy default-environment opt-out differs")
