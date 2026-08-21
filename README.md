# GitDock

[English](README.en.md)

GitDock 是一款面向 macOS 的 Git 桌面客户端，使用 Tauri、React、TypeScript 和 Rust 构建。它把常用 Git 工作流集中在一个紧凑的桌面界面中，同时为破坏性操作提供预览与确认。

## 功能

- 添加、异步克隆、初始化和管理本地仓库；克隆会显示进度并支持取消
- 通过文件名与路径同行的紧凑列表查看工作区状态，并以 unified 差异查看更改（删除行无行号，新增行显示新文件行号）；常用语言按需加载语法高亮
- 暂存、取消暂存或丢弃文件与单个代码块，提交更改；部分暂存文件在同一差异页显示已暂存与未暂存两侧，并支持批量选择；普通三阶段 UTF-8 文本冲突可在 Base / Current / Incoming 三栏中逐块选择并直接暂存
- 多选文件后通过下拉菜单批量暂存、取消暂存、丢弃或删除（含部分暂存文件）
- 在英文和简体中文界面之间切换，并记住语言选择
- 流畅滚动浏览跨页连续且窗口化渲染的提交拓扑图；提交后自动刷新图与提交列表，并可查看提交详情（元数据与变更文件列表）与单文件差异、执行 cherry-pick 与 revert
- 通过可折叠分组、置顶收藏组、新建空分组和拖拽排序管理仓库；状态色条与右上角数量标明工作区变更，键盘操作可完成同组排序
- 分组查看本地与远程分支，将远程分支直接检出为本地分支，并创建、切换、合并、变基、重命名和删除分支
- 管理标签、远程仓库、stash 与 submodule
- Fetch、Pull、Push，以及带 lease 的强制推送；Pull/Push 按钮直接显示待同步提交数，所有 Git 操作完成后显示短暂的结果提示
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
  - `src/App.tsx`：组件组合与全局布局，按领域使用 hooks 管理状态
  - `src/hooks/`：领域 hooks（仓库列表、工作区快照、历史、操作执行、日志）
  - `src/components/`：按面板拆分的 UI 组件（仓库列表、变更、历史、分支、Stash、对话框、Toast、命令面板）
  - `src/lib/`：纯逻辑工具（会话日志环形缓冲）
  - `src/types.ts`：跨组件共享类型与常量；`src/api.ts`：Tauri 命令封装
  - `src/App.test.tsx`：前端回归测试
- `src-tauri/src/`：Rust 后端，按职责拆分为模块
  - `lib.rs`：`AppState` 与 Tauri 命令注册；`summary.rs`：仓库摘要刷新；`repositories.rs`：仓库管理与配置；`history.rs`：历史与引用查询；`operations.rs`：Git 操作引擎与校验；`process.rs`：子进程、流与锁
  - `working_tree/`：工作区快照、差异、冲突缓存与过期校验；`repository_path.rs`：仓库相对路径校验
  - `git.rs`：Git 进程适配与其余读取查询；`models.rs`：共享数据类型；`store.rs`：配置持久化
- `src-tauri/icons/`：应用图标

## 安全说明

仓库路径和前端输入会在 Tauri 边界进行校验。内部冲突编辑器只接收后端生成的块 ID 与选择，并在写回前重新校验快照、索引阶段和工作区内容；不支持的冲突继续使用外部合并工具。删除文件、丢弃更改、强制推送等高风险操作保留确认流程；请在确认影响范围后再执行。配置按版本加载，保存前会把上一份有效配置备份为 `config.json.bak`。
