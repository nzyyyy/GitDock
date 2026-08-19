# GitDock

[中文](README.md)

GitDock is a macOS Git desktop client built with Tauri, React, TypeScript, and Rust. It brings common Git workflows into a compact desktop interface and previews destructive operations before they run.

## Features

- Add, asynchronously clone, initialize, and manage local repositories; clone streams progress and can be cancelled
- Inspect working-tree status in a compact filename-and-path row, and switch between unified and side-by-side diffs with on-demand highlighting for common languages
- Stage or unstage files and hunks and create commits; resolve ordinary three-stage UTF-8 text conflicts block by block in Base / Current / Incoming panes, then stage the result
- Select multiple files to stage or unstage them in one operation
- Switch between English and Simplified Chinese with a remembered preference
- Smoothly scroll through a windowed commit topology graph whose lanes continue across pages; commits refresh the graph and list automatically, with commit details (metadata and the changed-file list), per-file diffs, cherry-pick, and revert actions
- Organize repositories with collapsible groups, a pinned Favorites group, new empty groups, drag sorting, and keyboard ordering within a group
- Browse local and remote branches in separate groups, check out remote branches as local branches, then create, switch, merge, rebase, rename, and delete branches
- Manage tags, remotes, stashes, and submodules
- Fetch, pull, push, and force-push with lease; every Git operation shows a brief completion result
- Review affected paths and refs before sensitive Git operations run
- Enter Git operation details in validated in-app forms instead of browser prompts
- Explicitly export the bounded current-session Git log with URL credential redaction and no automatic persistence
- Use the `⌘K` / `Ctrl+K` command palette for stable workflows and repository actions; parameterized and dangerous operations retain their existing forms and impact previews
- Refresh all returns a fresh active summary plus session-cached inactive summaries immediately, then streams updates from at most four background Git processes

## Requirements

- macOS 14 or later
- Node.js 24 or later
- Git 2.30 or later
- A Rust toolchain for local development and packaging

## Development

```bash
npm install
npm run tauri dev
```

To run only the frontend:

```bash
npm run dev
```

## Testing

```bash
npm test
cargo test --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml --check
```

The 100,000-commit and 100,000-ignored-file performance checks are skipped by default and can be run explicitly:

```bash
cargo test --manifest-path src-tauri/Cargo.toml benchmarks_ -- --ignored --nocapture
```

Recorded benchmark results are in `docs/PERFORMANCE.md`.

## Packaging

```bash
npm run package
```

This keeps the TypeScript/Vite frontend output and copies the only macOS release bundle, the `.app`, to the repository's root `dist/` directory. It does not produce a `.dmg`:

```text
dist/assets/
dist/index.html
dist/GitDock.app
```

Both `dist/` and `src-tauri/target/` are generated directories and must not be committed.

## Project Structure

- `src/`: React/TypeScript frontend
  - `src/App.tsx`: component composition and global layout; state is managed per domain through hooks
  - `src/hooks/`: domain hooks (repository list, working-tree snapshot, history, operations, log buffer)
  - `src/components/`: pane-scoped UI components (repository list, changes, history, branches, stashes, dialogs, toasts, command palette)
  - `src/lib/`: pure utilities (session-log ring buffer)
  - `src/types.ts`: shared types and constants; `src/api.ts`: Tauri command wrappers
  - `src/App.test.tsx`: frontend regression tests
- `src-tauri/src/`: Rust backend, split by responsibility
  - `lib.rs`: `AppState` and Tauri command registration; `summary.rs`: repository summary refresh; `repositories.rs`: repository management and settings; `snapshot.rs`: working-tree snapshots/diffs/conflicts; `history.rs`: history and reference queries; `operations.rs`: Git operation engine and validation; `process.rs`: child processes, streams, and locks
  - `git.rs`: Git process wrapper and output parsing; `models.rs`: shared data types; `store.rs`: persisted settings
- `src-tauri/icons/`: application icons

## Safety

Repository paths and frontend input are validated at the Tauri boundary. The internal conflict editor accepts only backend-owned block IDs and choices, then revalidates the snapshot, index stages, and working-tree contents before writing; unsupported conflicts continue through external merge tools. High-risk actions such as deleting files, discarding changes, and force-pushing retain confirmation flows; review the affected scope before proceeding. Configuration is loaded by schema version, and the previous valid file is backed up as `config.json.bak` before saving.
