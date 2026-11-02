# HelixLane

[English](README.md)

使用 Rust / GPUI Kit 构建的原生 SSH、SFTP 工作台，采用 IDEA Islands 布局。首次启动为空工作区：创建资源，再打开终端、文件、tmux 或资源信息。

## 功能

- 资源管理命名连接配置；视图可独立「跟随」当前资源或「固定」目标。已提交的命令、传输、编辑回传和插件任务保留提交时的目标。
- 支持分屏、停靠、移动、分离视图和多个原生窗口。关闭视图或窗口可选择后台继续或取消其任务。最后一个窗口关闭后进程及私有启动通道仍运行；执行 `helixlane` 可重开。**退出 HelixLane 结束应用及其进程内任务**，与关闭窗口不同。
- SSH 支持密码、私钥、原始密钥 Agent、交互认证、主机身份确认、跳板链和本地/远程/SOCKS5 转发。可共享连接、独立连接或限制单通道。远程转发停止若无法确认，会关闭对应共享连接，避免遗留状态不明的监听端口。
- 真实 ANSI 终端及历史搜索，包含自动折行。Enter / Shift+Enter 或「下一个 / 上一个」切换匹配。设置提供等宽字体和字号，绘制、选择和输入法位置使用实测字宽；未安装字体使用原生等宽字体回退。
- SFTP 本地/远程导航、排序、隐藏文件、上传下载、经本地暂存的跨服务器复制、冲突策略、取消，以及外部编辑后明确确认回传。删除等操作确认已捕获目标。仅 SFTP 服务器不需要 Shell。
- 深色、浅色、跟随系统；中文、英文。实时偏好更改保留视图和会话。每视图保存非敏感路径、滚动、排序和隐藏文件选项；退出后不恢复进程会话或待处理任务。
- 受权限限制的 JavaScript GPUI Shell 子集，非无限制 Node 或插件市场。见 [插件说明](docs/plugin-host.md)。

## 构建与运行

支持编译器基线为 **Rust 1.99.1**。实际尝试 Rust 1.92 时，锁定的 `gpui-pre-util` 依赖因 `slice_as_array` 失败，因此不承诺旧编译器兼容。使用已提交的 Cargo.lock。

```sh
cargo build --locked --bins
cargo run --locked --bin helixlane
```

Linux 需要原生 X11/Wayland、字体、音频、图形库；Ubuntu 24.04 的准确依赖列表见 [CI](.github/workflows/native.yml)。macOS 需要 Xcode 命令行工具及兼容 SDK；Windows 需要 MSVC 与 Windows SDK。CLI 帮助和参数校验无需图形会话。

点击「创建资源」，选中资源后从工具栏打开终端或 SFTP。首次连接需确认主机身份。凭据可保存到系统钥匙串；失败会报告，不会降级明文保存。临时启动凭据仅保留内存，URI 禁止密码；CLI 密码可能被 OS 进程检查看到，建议交互认证。

```sh
helixlane open user@example.com --ssh --new-view
helixlane open sftp://user@example.com/work --new-window
helixlane 'helixlane://open?protocol=sftp&host=example.com&user=user&path=%2Fwork'
helixlane-putty -ssh -P 2222 -l user example.com
helixlane-winscp sftp://user@example.com/work /newinstance
```

兼容启动器只支持明确子集，非完整 PuTTY / WinSCP 替代。三个程序均支持 `--help`、`--version`。见 [启动及协议注册](docs/launch.md)、[SSH 网络](docs/ssh-networking.md)。Windows 使用 GUI 子系统避免资源管理器启动时出现控制台；可连接父控制台，保留重定向流和退出码。

快捷键：macOS 用 Cmd，其他平台用 Ctrl；Q 退出应用、Shift+N 新窗口、Shift+W 关闭窗口。终端复制/粘贴/搜索为 macOS Cmd+C/V/F，其他平台 Ctrl+Shift+C/V/F。

## 打包

```sh
cargo build --release --locked --bins
python3 scripts/package.py --platform linux
```

在对应原生平台用 `linux`、`macos` 或 `windows`。ZIP 含三个程序、URI 资源、文档、许可证、Cargo.lock 和 SHA256 清单。固定时间戳、顺序和权限使同一批二进制可重复打包；不代表不同 SDK 编译逐字节一致。包未签名，暂无公证或自动更新。

- Linux：解压后 `sh packaging/linux/install.sh /绝对路径/bin/helixlane`。
- macOS：复制 `HelixLane.app` 到 Applications 并注册 LaunchServices，或使用附带 `packaging/macos/install.sh`。
- Windows：PowerShell `packaging/windows/install.ps1 -Executable C:\绝对路径\bin\helixlane.exe`，仅当前用户。

## 兼容性

| 能力 | Linux | macOS | Windows |
|---|---|---|---|
| 原生桌面、三个程序 | 已实现；有桌面观察 | 已实现；需原生 CI | 已实现；需原生 CI |
| SSH / SFTP、跳板、转发 | 已实现 | 已实现 | 已实现 |
| 原始密钥 Agent | Unix 套接字 | Unix 套接字 | OpenSSH 命名管道 |
| 仅证书 Agent / Agent 转发 | 不支持 | 不支持 | 不支持 |
| 凭据保存 | Secret Service | Keychain | Credential Manager |
| 私有启动通道 | 用户 Unix 套接字 | 用户 Unix 套接字 | 仅所有者命名管道 |
| URI 注册 | 桌面条目 / xdg-mime | App / LaunchServices | HKCU 协议 |
| GUI、输入法、钥匙串、OS 链接 | GUI 已观察；其余桌面项待验证 | 待真实桌面验证 | 待真实桌面验证 |

已配置三平台构建、测试、原生传输与注册检查。**配置不等于执行通过；编译不等于 GUI / 输入法验证。** 见 [验证范围](docs/validation.md)。Linux 原生集成需真实 OpenSSH `sshd`、`sftp-server`、tmux 和特权分离；沙箱跳过不能计作通过。

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --features shell-tests -- -D warnings
cargo test --locked --features shell-tests -- --nocapture
cargo build --locked --bins --features shell-tests
python3 scripts/portable_smoke.py
python3 scripts/verify-russh-vendor.py
python3 scripts/verify-shell-vendor.py
python3 scripts/native_openssh.py
```

完整套件必须含 `shell-tests`，以运行真实 JS 权限、环境、卸载与上下文测试。`HELIXLANE_REQUIRE_NATIVE_INTEGRATION=1` 将 Unix IPC/tmux 跳过转为失败。完整 sshd 在普通套件中明确忽略，由需免密 sudo 的脚本执行。仅 `HELIXLANE_QA_FIXTURES=1` 启用 QA 示例，正式首次启动不会填充。

Apache-2.0。vendor 脚本严格验证上游来源和允许的有限差异。
