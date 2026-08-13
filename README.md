# GitDock

[English](README.en.md)

GitDock 是一款面向 macOS 的 Git 桌面客户端，使用 Tauri、React、TypeScript 和 Rust 构建。它把常用 Git 工作流集中在一个紧凑的桌面界面中，同时为破坏性操作提供预览与确认。

## 功能

- 添加、异步克隆、初始化和管理本地仓库；克隆会显示进度并支持取消
- 通过文件名与路径同行的紧凑列表查看工作区状态，并在 unified / side-by-side 差异视图间切换；常用语言按需加载语法高亮
- 暂存或取消暂存文件与 hunk，提交更改；普通三阶段 UTF-8 文本冲突可在 Base / Current / Incoming 三栏中逐块选择并直接暂存
- 多选文件后批量暂存或取消暂存
- 在英文和简体中文界面之间切换，并记住语言选择
- 流畅滚动浏览跨页连续且窗口化渲染的提交拓扑图；提交后自动刷新图与提交列表，并可查看提交差异、执行 cherry-pick 与 revert
- 通过可折叠分组、置顶收藏组和拖拽排序管理仓库；键盘操作可完成同组排序
- 分组查看本地与远程分支，并创建、切换、合并、变基、重命名和删除分支
- 管理标签、远程仓库、stash 与 submodule
- Fetch、Pull、Push，以及带 lease 的强制推送；所有 Git 操作完成后显示短暂的结果提示
- 在执行敏感 Git 操作前显示影响范围与确认信息
- 使用应用内校验表单完成 Git 操作输入，不依赖浏览器 prompt
- 显式导出当前会话的有界 Git 日志；导出时再次脱敏 URL 凭据且不会自动持久化
- 使用 `⌘K` / `Ctrl+K` 命令面板快速进入稳定工作流和仓库操作；参数与危险操作继续使用既有表单和影响预览
- Refresh all 立即返回当前仓库的新摘要与非活跃仓库的会话缓存，随后最多四路后台刷新并逐项更新

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

100,000 提交及 100,000 ignored 文件的性能检查默认跳过，可显式运行：

```bash
cargo test --manifest-path src-tauri/Cargo.toml benchmarks_ -- --ignored --nocapture
```

基准结果记录在 `docs/PERFORMANCE.md`。

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

仓库路径和前端输入会在 Tauri 边界进行校验。内部冲突编辑器只接收后端生成的块 ID 与选择，并在写回前重新校验快照、索引阶段和工作区内容；不支持的冲突继续使用外部合并工具。删除文件、丢弃更改、强制推送等高风险操作保留确认流程；请在确认影响范围后再执行。配置按版本加载，保存前会把上一份有效配置备份为 `config.json.bak`。
