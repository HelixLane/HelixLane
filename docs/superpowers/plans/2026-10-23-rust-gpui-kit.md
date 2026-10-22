# HelixLane Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 创建可启动、可继续开发的 Rust + GPUI Kit 桌面骨架。

**Architecture:** 单个 Cargo 二进制包。入口负责框架初始化和窗口生命周期，`app` 模块持有点击计数并渲染组件。

**Tech Stack:** Rust edition 2024，`gpui-kit = "0.8.0"`。

**Spec:** `docs/superpowers/specs/2026-10-23-rust-gpui-kit-design.md`

## Global Constraints

- Cargo 包名 `helixlane`，窗口标题 `HelixLane`。
- 保留 Apache-2.0 许可证；不添加业务模块或独立 GPUI 依赖。
- 使用默认后端、默认组件和资源；生成 Cargo.lock。
- 简单脚手架不添加重复实现的单元测试。

## Review Focus

- 初始化顺序：先注册资源和初始化 Kit，再创建窗口。
- 根视图：使用 Kit open_window，不重复包装 Root。
- 按钮交互：状态保存在视图中，点击后调用 notify。
- 生命周期：提供退出快捷键，关闭最后一个窗口时退出。
- 环境限制：区分静态检查通过和真实窗口交互验证。

## Task 1: 桌面骨架

**Files:** 创建 `Cargo.toml`、`src/main.rs`、`src/app.rs`、`.gitignore`、`README.md`，生成 `Cargo.lock`。

**Interfaces:** `app::HelixLane` 实现 `Default` 和 `Render`，入口使用 `cx.new` 构造。

- [x] 创建 Cargo 配置和忽略规则。
- [x] 创建入口：注册资源、初始化 Kit、退出动作和居中窗口。
- [x] 创建根视图：标题、说明、初始为零的计数、增加计数的按钮。
- [x] 编写中文 README，涵盖环境、运行和目录。
- [x] 执行 `cargo fmt --check`、`cargo check --locked`、`cargo clippy --locked --all-targets -- -D warnings`。
- [x] 如图形环境可用则尝试启动应用；记录验证范围。

## 验证记录

- 格式、编译检查和 Clippy 均退出 0。
- `cargo run --locked` 完成实际构建并启动 `target/debug/helixlane`，进程退出 0；未进行窗口截图或按钮点击观察。
- 独立代码复核未发现实质问题。
- 传递依赖 `block 0.1.6` 有未来 Rust 兼容性提示，未阻止当前构建。
