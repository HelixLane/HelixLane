# HelixLane Rust + GPUI Kit 初始项目

## 目标与范围

在当前仓库建立可继续开发的 Rust 原生桌面应用。用户指定 Rust + GPUI Kit，并同意最小骨架方案：窗口、按钮交互及运行说明。验收目标是在当前 macOS 环境完成编译检查，并能通过 `cargo run` 启动应用。

初始版本不包含业务功能、网络请求、数据库或应用发布配置。不改动现有许可证，不引入额外的前端工具链。

## 方案

- **最小桌面骨架（已选择）：** 单个 Cargo 二进制包，直接依赖 `gpui-kit`，文件少，便于后续扩展。
- **带侧边栏的应用模板：** 提供导航和内容区域，但会提前确定尚无业务需求的布局；本次不采用。

以下为实施默认选择：Cargo 包名 `helixlane`，窗口标题 `HelixLane`，Rust edition 2024；按照官方安装文档使用 `gpui-kit = "0.8.0"` 的默认后端和默认组件、图标资源，不单独添加 GPUI 依赖。以实际依赖解析结果生成并保留 `Cargo.lock`。

## 结构与行为

- `Cargo.toml`：应用元信息、Apache-2.0 许可证声明及 GPUI Kit 依赖。
- `src/main.rs`：应用入口、框架初始化、资源注册及窗口创建。
- `src/app.rs`：根内容视图及点击计数状态。初始显示应用名称、说明、计数 0 和一个 GPUI Kit 按钮；每次点击增加计数并通知视图刷新。
- `.gitignore`：忽略 Rust 构建产物及本地 IDE 文件。
- `README.md`：中文环境准备、运行命令、检查命令、目录说明及官方文档链接。

按官方流程先调用 `init`，再通过 Kit 的 `open_window` 包装根内容视图；不重复创建 `Root`。窗口创建失败时给出明确的错误信息。状态仅保存在内存，重新启动后归零。窗口大小、文字及样式由实施时确定，保持最小示例规模。

## 验证

运行 `cargo fmt --check`、`cargo check --locked` 和 `cargo clippy --locked --all-targets -- -D warnings`，确保依赖解析、类型与格式正确。该项目仅有简单 UI 状态，不为脚手架添加重复实现的单元测试。

如环境允许，启动应用检查窗口与按钮；未实际观察到的图形交互不宣称已验证。若依赖下载或本机工具链阻止编译，记录具体原因及可重试命令。

## 参考

- https://gpui-kit.com/docs/installation/
- https://gpui-kit.com/docs/getting-started/
