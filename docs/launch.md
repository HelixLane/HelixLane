# Native launch integration

All entry paths use one validated request and the existing native SSH/SFTP panes. Build with `cargo build --bins`; keep `helixlane`, `helixlane-putty` and `helixlane-winscp` in the same directory. `--help` and `--version` work without a display. Arguments are already tokenized by the caller: no shell splitting or shell execution occurs.

```
helixlane open alice@server.example --port 2222 --password 'one argument'
helixlane open sftp://alice@server.example/var/log --auth-none --new-view
helixlane open --resource 1 --sftp --path /var/log --activate
helixlane 'helixlane://open?protocol=sftp&host=server.example&user=alice&path=%2Fvar%2Flog'
helixlane-putty -ssh -P 2222 -l alice -pw 'one argument' server.example
helixlane-winscp 'sftp://alice:encoded%3Apassword@server.example/a%20b' /sessionname=Work
```

The native grammar accepts `open <host|user@host|ssh://...|sftp://...>` or `open --resource ID`. Options: `--ssh`, `--sftp`, `--user`, `--port`, `--password`, `--key`, `--auth-none`, `--path`, `--name`, `--activate`, `--new-view`, `--new-window`. Conflicting or duplicate options fail. A saved resource owns its username, port and authentication; these cannot be overridden through an external request.

The own URI accepts only `protocol=ssh|sftp`, `host`, `user`, `port`, `path`, `name`, `mode=activate|new-view|new-window`, or `resource=ID`. A resource excludes host/user/port. Encode UTF-8 query fields with percent encoding (`%20` for space); duplicate/unknown fields, invalid percent sequences, invalid Unicode and controls fail with sanitized bilingual diagnostics. Passwords/userinfo are forbidden in own URIs and native SSH/SFTP URLs; native callers use `--password`. WinSCP compatibility alone accepts encoded legacy URL credentials. URI handlers may expose URLs in OS history, so never put a password in a URI.

PuTTY supports host, `user@host`, `-ssh`, `-P`, `-l`, `-pw`. `-load`, `-i`, remote commands and other protocols are explicitly unsupported. WinSCP supports SFTP URL, `/sessionname=`, `/newinstance`; scripts, commands, saved-session import and other options are unsupported. New-window intent is preserved but this build reports that new windows are unavailable, without creating a resource/view. Activate reuses an existing native widget. Plain invocation focuses the primary window.

Usable authentication starts through the existing visible host-key trust flow. Missing username/authentication opens the native prompt. Decoded SFTP paths reach the file pane and survive authentication edits. Secrets stay in zeroizing memory, are redacted in Debug, and proxy cold starts use only `--launch-stdin` in child argv with a private inherited stdin pipe. User-supplied CLI passwords can still appear in the caller's original process command line. External connection profiles and newly-created views for unsaved connections are temporary, cannot remember credentials, and are excluded at the central settings snapshot boundary. Saved Follow views retain their identity and persistence when temporarily following an external resource. New views opened for a saved resource ID remain persistent. Temporary transfer endpoints never enter the disk journal.

Single instance: Unix uses an owned non-symlink `0700 /tmp/helixlane-EUID` directory, `0600` lock/socket, nofollow/CLOEXEC exclusive file lock and peer UID checks on both ends. Only the lock owner cleans a stale socket. Windows uses a SID-scoped named pipe with a protected owner DACL, local clients only, first-instance ownership and server-process SID verification before credential delivery. Only `ERROR_PIPE_BUSY` retries; absence returns immediately, security/unknown errors fail immediately, and one absolute two-second transaction deadline covers retry, verification and payload. Frames are limited to 64 KiB, pending requests and concurrent transactions to 32.

Some container environments prohibit AF_UNIX. Debug builds only support explicit `HELIXLANE_TEST_IPC_TCP=0` (or a loopback port) for a same-executable-tree test fixture. It uses private owner-only token/port files, a fresh random 32-byte token and mutual role-separated nonce/HMAC-SHA256 proofs with constant-time verification. There is no production fallback. Linux is validated in this environment; native macOS association and Windows pipe security/concurrency require platform CI.

Settings: explicit `HELIXLANE_SETTINGS` overrides the file path. Defaults use Windows APPDATA (or USERPROFILE/AppData/Roaming), macOS HOME/Library/Application Support, or an absolute Linux XDG_CONFIG_HOME (otherwise HOME/.config), under `helixlane/settings.sqlite`. Missing OS roots produce an explicit startup error; there is no implicit cwd storage.

Platform association installation is opt-in:

* Linux: `sh packaging/linux/install.sh /absolute/path/helixlane` installs a per-user desktop entry and registers the scheme. The generator can be tested without installing: `python3 packaging/linux/generate_desktop.py /absolute/path/helixlane`. Literal percent signs in the executable become `%%`; `%u` remains a separate URL argument.
* macOS: `sh packaging/macos/install.sh /absolute/binaries/directory [destination.app]` assembles and registers an app bundle.
* Windows: `powershell -File packaging/windows/install.ps1 -Executable C:\absolute\helixlane.exe` registers the scheme for the current user.

Run `python3 scripts/launch_smoke.py` after building all binaries. It validates help/version, sanitized failures, invalid OS UTF-8, and proxy stdin delivery against a harmless sibling stub. It never runs an association installer.

`python3 scripts/desktop_entry_smoke.py` validates the actual Linux generator through GLib key-file/argument parsing, including spaces, backslash, quote, dollar, backtick, literal percent and separate `%u`, without installing an association. The current container GLib desktop launcher rejects an executable path containing `%%` before field expansion; the generator retains the required desktop-entry escaping. Native desktop integration with percent-containing installation paths remains a platform-validation limitation.
