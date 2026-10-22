# HelixLane

使用 Rust + [GPUI Kit](https://gpui-kit.com/) 构建的原生桌面工作台，
当前提供类似 IDEA Islands 的深色固定区域布局预览。

## 布局预览

- 左侧 Project / Search 按钮切换或收起工具面板。
- Project 中可展开目录、打开示例文件；Search 可按路径筛选文件。
- 中央标签页可切换和关闭，显示带基础词法着色的只读源码。
- 左侧下方 Terminal / Problems 按钮可展开或收起底部工具窗口。
- 底部支持 Terminal / Problems / Output 标签切换。
- 拖动区域之间的分隔条可调整文件树宽度和底部窗口高度。
- 右侧按钮可显示当前文件信息。

示例文件为编译时嵌入的项目文件。此版本用于评估布局：编辑区只读，终端为预览，
尚未连接语言服务、编译诊断或真实 shell；布局暂不跨启动保存。

## 开发环境

- Rust 1.92 或更高版本，推荐当前 stable 工具链。
- macOS 15 或更高版本，以及 Xcode Command Line Tools / Xcode。
- 其他平台的系统依赖参见 [GPUI Kit 安装文档](https://gpui-kit.com/docs/installation/)。

```sh
rustc --version
cargo --version
xcode-select -p
```

## 运行

在仓库根目录执行：

```sh
cargo run --locked
```

首次构建需要下载并编译 GPUI 的依赖，耗时会较长。
关闭最后一个窗口即可退出；macOS 也可使用 Cmd+Q，其他平台使用 Ctrl+Q。

构建发布版本：

```sh
cargo build --release --locked
```

## 检查

```sh
cargo fmt --check
cargo check --locked
cargo clippy --locked --all-targets -- -D warnings
```

## 目录

```text
src/
  main.rs   # 框架初始化、窗口与退出动作
  app.rs    # 工作台、工具窗口与交互状态
  editor.rs # 示例文件与只读源码展示
  theme.rs  # Islands 视觉常量与容器
Cargo.toml  # 应用配置与 GPUI Kit 依赖
Cargo.lock  # 固定应用依赖版本
```

GPUI Kit 默认包含 GPUI、组件和图标资源。应用通过 `gpui_kit` 使用这些 API，
无需额外添加独立的 `gpui` 依赖。更多示例见 [官方入门指南](https://gpui-kit.com/docs/getting-started/)。

## 许可证

[Apache-2.0](LICENSE)
