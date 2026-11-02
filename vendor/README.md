# russh 0.65.0 compatibility patch

Upstream: https://crates.io/crates/russh/0.65.0 (Apache-2.0; license retained in `russh/LICENSE`).
Published crate SHA-256: `3bb77a4b259312b2abbc541bca1c13cb4e9b61c560a7fd894068c3b92e0c9dc3`.

GPUI Shell 0.8.1 requires quickjs-jit-stdlib 0.14.1, whose Rust crypto feature requires RSA 0.10.0-rc.19. Published russh 0.65.0 pins RSA rc.18 and PKCS#1 rc.4; Cargo cannot resolve these with the shell. This local patch changes the normalized `Cargo.toml` pins to RSA rc.19 and PKCS#1 rc.5. RSA rc.19 itself advances PKCS#1 to rc.5 and signature to 3.0. The only Rust source change is `pkcs1::RsaPrivateKey::try_from` → `pkcs1::RsaPrivateKeyRef::try_from` in `src/keys/format/pkcs8.rs`: rc.5 made the type generic and the new alias selects the same borrowed `UintRef` representation used by rc.4. No crypto backend, enabled algorithm, parsing/signing behavior, or public SSH API changes. The original manifest remains unchanged for provenance.

`python3 scripts/verify-russh-vendor.py` verifies all 90 published files against recorded SHA-256 hashes, reversing only these exact three edits in memory. `.cargo-ok` is a local Cargo cache marker, not a published source file, and is omitted.

Keep this patch until an upstream russh release accepts these crypto versions; replace it with the official compatible release after SSH/SFTP and RSA authentication tests pass. This prerelease dependency compatibility patch is explicitly covered by the workbench's RSA private-key load/authentication test and complete SSH/SFTP tests.
