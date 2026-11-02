# SSH/SFTP Workbench Implementation Plan

> **For agentic workers:** Use superpowers:subagent-driven-development task by task. User approved autonomous execution on 2026-11-02; do not request repeated stage approvals.

**Goal:** Turn the existing preview into a themed bilingual native SSH/SFTP workbench.
**Architecture:** A serializable resource/view/layout model drives GPUI views; asynchronous services own network sessions and transfer tasks independently of their presentation. One executable accepts GUI, CLI and URI requests; thin compatibility binaries forward legacy syntax.
**Tech Stack:** Rust 2024, GPUI Kit 0.8.0, mature Rust SSH/SFTP and terminal libraries, versioned settings and OS credential storage.
**Spec:** `docs/superpowers/specs/2026-11-02-ssh-sftp-workbench-design.md`

## Global Constraints

- Keep Rust + GPUI Kit and Apache-2.0; do not convert to a web app.
- UI first: IDEA Islands, complete Dark/Light/System themes, English/Chinese, no online fonts.
- Resources own connection profiles; view instances own Follow/Fixed and Background/While Visible.
- Moving a view preserves its ID and session; submitted tasks capture immutable targets.
- No silent host-key acceptance or plaintext credential persistence.
- Only SSH/SFTP protocols required now; preserve the remaining approved workbench scope.
- User authorizes implementation, fixes, tests and repository publication until the goal is met.

## Review Focus

1. Rapid Follow switches must reject late data and cannot redirect tasks.
2. Closing/moving views must not cancel transfers or accidentally disconnect shared sessions.
3. Invalid or truncated settings must recover without losing credential references.
4. Legacy launch quoting, URL escaping and secret redaction must survive adversarial input.
5. SSH host-key changes, SFTP-only hosts and interrupted transfers must fail clearly.

## Task 1: Workbench model, themes and UI

Files: create `src/workspace.rs`, `src/settings.rs`, `src/i18n.rs`, `src/lib.rs`; modify `src/app.rs`, `src/theme.rs`, `src/main.rs`, `Cargo.toml`.
Interfaces: serializable `Resource`, `ViewInstance`, `ToolKind`, `TargetMode`, `RunPolicy`, layout tree and settings. UI operations mutate the model; future services address instances by ID.
- [x] Tests first: view move preserves ID; fixed views ignore follow; late generations reject; layout roundtrip and corrupt settings fallback.
- [x] Implement model and versioned settings; credentials remain references.
- [x] Replace code-demo default with resource workbench, resource editor, Services resource/type tree, central tabs/splits, four-side tools and move commands; use Kit docking if compatible.
- [x] Dark/Light/System and Chinese/English UI, persisted settings; meaningful empty/error states.
- [x] Verify fmt/check/model tests and visually inspect when desktop available; record exact gaps, commit.

## Task 2: SSH service and real terminal

Files: create `src/ssh/`, `src/terminal/`, credential and host-key modules; wire app service events.
Interfaces: `ConnectionProfile` inputs; sessions identified independently from views; terminal PTY input/resize/output and cancellable exec API.
- [ ] Tests first: host key TOFU/change, target snapshot and stale result rejection; terminal ANSI/alternate screen handling.
- [ ] Integrate russh or validated equivalent, authentication/keychain, host-key prompts, nonblocking session lifecycle and jump/forward extension.
- [ ] Integrate terminal state machine, GPUI rendering/input, selection/clipboard/search, resize and connection UI states.
- [ ] Verify real OpenSSH session locally including errors; run suite, commit.

## Task 3: SFTP, transfer jobs and resource tools

Files: create `src/sftp/`, `src/tasks/`, file browser views and tmux tool; modify app wiring.
Interfaces: independent `TransferManager` with captured source/target; SFTP list/stat/mkdir/rename/remove/read/write APIs.
- [ ] Tests first: temp upload cancellation, conflict detection, fixed four-host views, no-follow symlinks, retries and task/view independence.
- [ ] Implement local/remote browser, operations, permissions, conflict dialogs, progress/cancel and concurrency.
- [ ] Add safe external editing and tmux operations with explicit target terminal; follow and visibility policies apply to refresh only.
- [ ] Real OpenSSH/SFTP integration tests and UI verification, commit.

## Task 4: Native CLI, IPC, URI and compatibility binaries

Files: create `src/launch/`, `src/bin/helixlane-putty.rs`, `src/bin/helixlane-winscp.rs`, platform registration assets.
Interfaces: a single validated `OpenRequest`; main parses before GUI; IPC transfers requests without re-exposing secrets.
- [ ] Tests first: native/resource/URI parity, PuTTY port/user/password parsing, WinSCP escaped URLs/path/sessionname, unknown flags and command-injection strings.
- [ ] Implement help/version, bounded per-user IPC, opening/focusing/new-window requests and thin proxies.
- [ ] Add protocol registration/install assets for three platforms; explicit compatibility matrix.
- [ ] Run parser/IPC tests and smoke executable CLI, commit.

## Task 5: Plugin seam, advanced networking and release verification

Files: plugin capability module/example, SSH jump/forward support, CI workflows, packaging and bilingual README.
- [ ] Verify gpui-shell host API against installed Kit; implement one permission-scoped example with unload cleanup.
- [ ] Test jump forwarding and single-channel restrictions; finish independent-window view migration.
- [ ] Add Linux/macOS/Windows build/test CI and reproducible packaging instructions.
- [ ] Full tests/fmt/Clippy, actual GUI theme/language/four-pane smoke, independent branch review and fixes.
- [ ] Publish reviewed branch/PR; report remaining limits truthfully; do not label partial phases as final completion.

## Preflight and execution rulings

Tasks 1–5 share model interfaces and app wiring, so implementations run sequentially. Task 1 defines stable IDs and schema; subsequent tasks adapt to the checked-in interface rather than creating parallel models. No detected spec contradictions. Toolchain and OS package installation can run alongside implementation as environment preparation. Plan approvals are covered by the user's explicit authorization to subsequent automated execution.
