# GitDock

[English](README.en.md)

GitDock 是一款面向 macOS 的 Git 桌面客户端，使用 Tauri、React、TypeScript 和 Rust 构建。它把常用 Git 工作流集中在一个紧凑的桌面界面中，同时为破坏性操作提供预览与确认。

## 功能

- 添加、克隆、初始化和管理本地仓库
- 查看工作区状态、文件差异以及暂存/未暂存更改
- 暂存或取消暂存文件与 hunk，提交更改，处理冲突
- 多选文件后批量暂存或取消暂存
- 在英文和简体中文界面之间切换，并记住语言选择
- 浏览提交历史和分支图，查看提交差异，执行 cherry-pick 与 revert
- 创建、切换、合并、变基、重命名和删除分支
- 管理标签、远程仓库、stash 与 submodule
- Fetch、Pull、Push，以及带 lease 的强制推送
- 在执行敏感 Git 操作前显示影响范围与确认信息

## 环境要求

- macOS 14 或更高版本
- Node.js 24 或更高版本
- Git 2.30 或更高版本
- Rust 工具链（本地开发或打包时需要）

## 开发

```bash
npm install
npm run tauri dev
```

仅启动前端：

```bash
npm run dev
```

## 测试

```bash
npm test
cargo test --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml --check
```

## 打包

```bash
npm run package
```

该命令保留 TypeScript/Vite 前端编译产物，并将唯一的 macOS 发布包 `.app` 复制到仓库根目录的 `dist/`。不会生成 `.dmg`：

```text
dist/assets/
dist/index.html
dist/GitDock.app
```

`dist/` 和 `src-tauri/target/` 均为生成目录，不应提交。

## 项目结构

- `src/`：React/TypeScript 前端
- `src-tauri/src/`：Rust 后端、Git 命令与 Tauri 接口
- `src-tauri/icons/`：应用图标
- `src/App.test.tsx`：前端回归测试

## 安全说明

仓库路径和前端输入会在 Tauri 边界进行校验。删除文件、丢弃更改、强制推送等高风险操作保留确认流程；请在确认影响范围后再执行。
