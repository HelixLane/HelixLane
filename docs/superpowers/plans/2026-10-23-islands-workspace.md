# Islands Workspace Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans for inline execution.

**Goal:** 展示固定区域、可交互的 Islands 深色工作台。

**Architecture:** 工作台持有标签和面板显隐状态；GPUI Kit 管理分隔条及输入框；独立 editor/theme 模块承载源码展示和视觉常量。

**Tech Stack:** Rust 2024，现有 gpui-kit 0.8.0。

**Spec:** `docs/superpowers/specs/2026-10-23-islands-workspace-design.md`

## Constraints and review focus

固定区域，无自由停靠；源码和终端只读预览；不写业务文件；不保存布局；处理关闭当前/全部标签和面板恢复；兼顾最小窗口尺寸及 macOS 标题栏。

## Task 1: 可运行预览

- [x] 增加 theme 和 editor 模块，替换计数页面为工作台。
- [x] 接通文件树、筛选、标签关闭切换、底部标签、工具区域显隐和分隔条。
- [x] 更新入口尺寸和标题栏，更新运行说明。
- [x] 运行格式、编译、Clippy，构建启动并尝试针对应用窗口截图。
- [x] 独立复核功能和外观限制，记录结果。

## 验证记录

- cargo fmt、cargo check --locked 和 cargo clippy --locked --all-targets -- -D warnings 通过。
- cargo run --locked 启动成功，用户确认能够看到程序界面。
- 独立复核发现并修正：不同面板组合使用独立尺寸状态，避免按索引串位；取消 fixed 标志，允许缩窗时面板收缩。
- 窗口截图不可用；用户直接评估原生程序外观，无截图文件。
- 源码只读展示，终端和诊断为预览；布局不跨启动保存。
