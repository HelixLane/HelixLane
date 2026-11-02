# Native resource plugin host

Open **Resource plugin** from the left tool rail, then **Enable example plugin**. The example is a real gpui-shell `ScriptView` inside the existing native ToolPanel/DockArea, with the same view ID, Follow/Fixed target binding, placement and run-policy controls as builtin tools. Enable state and placement restore through the existing versioned SQLite/settings and dock snapshot paths. **Unload** revokes host access immediately; **Reload** retries a failed script. Script build errors appear in the panel. Language and theme follow the workbench.

`plugins/resource-overview/main.js` is bundled with the executable. This first host accepts this bundled example; it does not install remote code, read plugin manifests or fetch plugin dependencies. Changes to the selected resource create a new immutable context and replace the previous script instance. The example has no background refresh work. WhileVisible views capture updates on their next visible render; existing native background sessions/transfers continue unchanged.

## Ownership and permissions

`ToolProvider` creates `ToolInstance` for every native adapter and plugin. Native Terminal/SFTP/tmux profile bindings pass through `native_target`, which exists only for trusted builtin providers. They retain their existing shared SSH/session/transfer services and stable identity. Only the `helixlane:resource` host module crosses into JavaScript. It returns a captured view ID/generation and optional resource ID/name/host/port, plus localized labels. It exposes no passwords, usernames, authentication configuration, private-key paths, SecretRefs, full settings, CLI-only profiles or CLI-only view targets. The bundled view freezes both snapshot objects; changing a returned JS object cannot mutate the captured host snapshot.

Each script instance has its own isolated VM and `policy::Policy`. The resource-summary host grant is explicit. OS filesystem read/write, process execution, network, environment, clipboard, persistent storage and exit are denied by default. All exported functions check a revocable lease. ToolInstance cleanup hooks own host subscriptions/task cancellation; dropping ScriptView releases the shell's own retained callbacks, timers and async tasks. No shared global grants or exported modules are installed. A minimal vendored shell patch adds a per-policy opt-out from implicit environment defaults, including credential-bearing proxy URLs; every plugin mount opts out without mutating the process environment.

GPUI Shell 0.8.1 only exposes default-policy `load_application`/`mount_application` publicly. `CreationPolicy` lends a per-plugin policy for these **synchronous** calls and restores the previous policy on success, error or unwind. There is no await, event-loop yield or reentrant host UI operation in this section. ScriptViews capture their own `Rc<Policy>`; later renders, callbacks and async resumptions retain that authority. Tests exercise coexistence after changing the creation default and unloading tasks.

The host writes the embedded source to a private temporary bundle solely for the shell's file-based public loader, disables editor-file generation, and removes the bundle when its owner drops. It does not grant the script filesystem access.

## Verification

- `cargo test --features shell-tests --lib tools::` exercises captured/sanitized contexts, denial, restoration, real VM/native element snapshots, async policy reentry and unload cleanup. GPUI test-platform snapshots are not native OS GUI proof.
- `cargo test --features shell-tests` runs the full suite, including real loopback SSH/SFTP and RSA SHA2 authentication.
- `python3 scripts/verify-shell-vendor.py` validates the per-policy environment isolation patch.
- `python3 scripts/verify-russh-vendor.py` checks the minimal forward crypto compatibility patch described in `vendor/README.md`.
- `cargo clippy --features shell-tests --all-targets -- -D warnings`, `cargo build`, `cargo fmt --check`.

Actual native desktop QA must separately open the panel, enable it, move/split it, switch Follow/Fixed targets, unload/reload, restore placement after restart, and inspect English/Chinese plus light/dark appearance.
