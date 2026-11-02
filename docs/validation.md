# Native validation scope / 原生验证范围

`.github/workflows/native.yml` pins Rust 1.99.1 on Ubuntu 24.04, macOS 15 and Windows 2025. Full tests and lint include `--features shell-tests`; packages include three native executables and URI assets.

The actual `cargo +1.92.0 check --locked --all-targets --features shell-tests` failed in locked `gpui-pre-util 0.3.10`, `src/lib.rs:575`, E0658 (`slice_as_array`). The baseline was raised to the installed supported compiler, not inferred as the lowest theoretically compatible compiler.

| Gate | Establishes | Does not establish |
|---|---|---|
| Full Rust suite | Models, encrypted fixtures, SFTP, real JS policy/lifecycle | Native GUI, IME or keychain prompts |
| Unix IPC / Windows pipes | Actual owner-protected transport and queue behavior | Desktop URL dispatch |
| Linux `native_openssh.py` | Full sshd, password/key/KBI/raw-key agent, SFTP-only, jump, local/wildcard remote forward | macOS agent/Keychain or Windows OpenSSH service |
| Native URI registration | Registry/LaunchServices/xdg registration and argument structure | Manual activation from another desktop app |
| CLI and PE smoke | Real help/output/redaction/exit codes; Windows GUI subsystem | Explorer appearance or interactive terminal |
| ZIP manifest / repeat assembly | Three binaries, native assets, identical assembly bytes | Signing/notarization or cross-compiler reproducibility |

The full sshd test is explicitly ignored until provisioned; Linux CI runs it through the provisioning script. Local capability guards print SKIP; CI sets `HELIXLANE_REQUIRE_NATIVE_INTEGRATION=1` on Unix to fail those guards. Windows exercises actual named pipes; tmux is not required on native Windows.

The sandbox can run encrypted loopback fixtures and Linux GUI, but denies some AF_UNIX/sshd privilege-separation operations. Such skips are limitations, not native passes. macOS/Windows GUI, IME, keychain and browser-to-app URI activation remain unobserved until tested on those desktops. A configured CI workflow is pending external execution evidence.

Desktop acceptance: fresh empty launch, real four-pane SFTP and smaller two-pane windows in Dark/Light and Chinese/English, long endpoint and Follow/Fixed control reachability, file Actions and captured destructive confirmations, live theme/font/language while connected, history search beyond the viewport, wide/combining characters and native IME, persisted paths/browser flags. Exercise Follow/Fixed, WhileVisible/Background, hide/show, moves, background work with no windows, CLI reopen and explicit Quit separately.

中文：原生 CI 与桌面验收提供不同证据。沙箱跳过、已配置工作流、编译通过不能替代原生集成或 GUI / 输入法观察。完整 sshd 必须由脚本明确执行；macOS / Windows 桌面观察需独立记录。
