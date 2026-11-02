# HelixLane

[简体中文](README.zh-CN.md)

A native Rust / GPUI Kit SSH and SFTP workbench with an IDEA Islands layout. Start with an empty workspace, create a resource, then open terminals, files, tmux or resource information.

## Working with connections

- Resources own named connection profiles. Views independently follow the active resource or stay fixed. Submitted commands, transfers, editor uploads and plugin jobs retain their captured destination.
- Split, dock, move and detach views into multiple windows. Closing a view or window offers continuing work in the background or cancelling its owned work. Closing the last window leaves the process and private launch channel running; reopen with `helixlane`. **Quit HelixLane ends the application and its in-process work**, distinct from Close window.
- SSH supports passwords, private keys, raw-key agents, keyboard-interactive authentication, host-key approval, jump routes and managed local/remote/SOCKS5 forwarding. Channels may share a connection, use an independent connection or enforce one channel. An unconfirmed remote-forward stop closes its shared transport to avoid leaving an unknown listener.
- ANSI terminal with retained scrollback search, including soft-wrapped lines. Enter / Shift+Enter or Next / Previous navigate matches. Settings offers monospace families and sizes; drawing, selection and IME geometry use the measured font. Missing families use the native monospace fallback.
- SFTP local/remote browsers, path navigation, sort/hidden-file preferences, upload/download, server-to-server copies through local staging, conflict choices, cancellation and external editing with upload confirmation. Destructive operations require confirmation against captured targets. SFTP-only hosts need no shell.
- Dark, Light and System themes; English and Chinese. Live preferences retain view/session entities. Non-sensitive paths, scroll positions, sort and hidden-file preferences persist per view. Sessions and pending jobs do not survive application exit.
- A capability-limited JavaScript plugin host implements the documented GPUI Shell subset, not an unrestricted Node runtime or marketplace. See [plugin host](docs/plugin-host.md).

## Build and run

Supported compiler baseline: **Rust 1.99.1**. An actual Rust 1.92 build failed in the locked `gpui-pre-util` dependency on `slice_as_array`; older compiler compatibility is not promised. Always use the committed Cargo.lock.

```sh
cargo build --locked --bins
cargo run --locked --bin helixlane
```

Linux requires native X11/Wayland, font, audio and graphics development libraries; the exact Ubuntu 24.04 package list is in [CI](.github/workflows/native.yml). macOS needs Xcode command line tools and a compatible SDK. Windows needs MSVC build tools and Windows SDK. CLI help and validation work without a graphical session.

Click **Create resource**, select it and open a terminal or SFTP view from the tool strip. First contact requires explicit host identity approval. Credentials may use the system keychain; failures are reported, never replaced with plaintext persistence. Temporary launch credentials stay in memory. URI passwords are forbidden. CLI password arguments can be visible to OS process inspection; prefer interactive authentication.

```sh
helixlane open user@example.com --ssh --new-view
helixlane open sftp://user@example.com/work --new-window
helixlane 'helixlane://open?protocol=sftp&host=example.com&user=user&path=%2Fwork'
helixlane-putty -ssh -P 2222 -l user example.com
helixlane-winscp sftp://user@example.com/work /newinstance
```

Compatibility launchers support a bounded subset, not complete PuTTY/WinSCP replacements. All three executables support `--help` and `--version`. See [launch contract and URI installation](docs/launch.md) and [SSH networking](docs/ssh-networking.md). Windows uses the GUI subsystem to avoid a new console from Explorer, attaches to a parent console when available and preserves redirected streams/exit codes.

Keyboard: Cmd+Q / Ctrl+Q quits; Cmd+Shift+N / Ctrl+Shift+N creates a window; Cmd+Shift+W / Ctrl+Shift+W closes a window. Terminal copy/paste/find uses Cmd+C/V/F on macOS and Ctrl+Shift+C/V/F elsewhere.

## Packages

```sh
cargo build --release --locked --bins
python3 scripts/package.py --platform linux
```

Use `linux`, `macos` or `windows` on the corresponding native runner. Each ZIP includes all three binaries, URI assets, documentation, license, Cargo.lock and SHA256 manifest. Assembly uses fixed timestamps, modes and ordering: reproducible packaging of the same binaries, not cross-SDK bit-identical compilation. Packages are unsigned; signing/notarization and automatic updates are not implemented.

- Linux: extract and run `sh packaging/linux/install.sh /absolute/path/to/bin/helixlane`.
- macOS: copy `HelixLane.app` to Applications and register with LaunchServices, or use `packaging/macos/install.sh` against a directory containing the three binaries.
- Windows: PowerShell `packaging/windows/install.ps1 -Executable C:\absolute\path\bin\helixlane.exe`; registration is per user.

## Feature and compatibility matrix

| Capability | Linux | macOS | Windows |
|---|---|---|---|
| Native desktop, three binaries | Implemented; desktop observed | Implemented; native CI required | Implemented; native CI required |
| SSH/SFTP, jump routes, managed forwarding | Implemented | Implemented | Implemented |
| Raw-key agent | Unix socket | Unix socket | OpenSSH named pipe |
| Certificate-only agent / agent forwarding | Unsupported | Unsupported | Unsupported |
| Credential storage | Secret Service | Keychain | Credential Manager |
| Private launch channel | Per-user Unix socket | Per-user Unix socket | Owner-only named pipe |
| URI registration | Desktop entry / xdg-mime | App / LaunchServices | HKCU protocol |
| GUI, IME, keychain, OS-link verification | GUI observed; other desktop checks pending | Pending desktop validation | Pending desktop validation |

Configured CI builds, lints and tests all three platforms, including native transport and registration checks. **Workflow configuration is not a completed run; compilation is not GUI/IME validation.** See [validation scope](docs/validation.md). Native Linux integration requires actual OpenSSH `sshd`, `sftp-server`, tmux and privilege separation. Sandbox skips must not count as native integration passes.

## Validation

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --features shell-tests -- -D warnings
cargo test --locked --features shell-tests -- --nocapture
cargo build --locked --bins --features shell-tests
python3 scripts/portable_smoke.py
python3 scripts/verify-russh-vendor.py
python3 scripts/verify-shell-vendor.py
# Linux with passwordless sudo and disposable test users:
python3 scripts/native_openssh.py
```

`shell-tests` is required for real JavaScript permissions, environment, unload and context tests. `HELIXLANE_REQUIRE_NATIVE_INTEGRATION=1` makes Unix IPC/tmux guards fail instead of skipping. Full sshd is explicitly ignored in the portable suite and run by the provisioning script. QA fixtures require `HELIXLANE_QA_FIXTURES=1`; they never populate production first launch.

Apache-2.0. Narrow vendor differences are fingerprinted by the verification scripts; original dependency licenses remain in `vendor/`.
