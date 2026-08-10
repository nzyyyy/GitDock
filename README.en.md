# GitDock

[中文](README.md)

GitDock is a macOS Git desktop client built with Tauri, React, TypeScript, and Rust. It brings common Git workflows into a compact desktop interface and previews destructive operations before they run.

## Features

- Add, asynchronously clone, initialize, and manage local repositories; clone streams progress and can be cancelled
- Inspect working-tree status in a compact filename-and-path row, and switch between unified and side-by-side diffs with on-demand highlighting for common languages
- Stage or unstage files and hunks, create commits, and resolve conflicts
- Select multiple files to stage or unstage them in one operation
- Switch between English and Simplified Chinese with a remembered preference
- Smoothly scroll through a windowed commit topology graph whose lanes continue across pages; commits refresh the graph and list automatically, with commit diff, cherry-pick, and revert actions
- Organize repositories with collapsible groups, a pinned Favorites group, drag sorting, and keyboard ordering within a group
- Browse local and remote branches in separate groups, then create, switch, merge, rebase, rename, and delete branches
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
- `src-tauri/src/`: Rust backend, Git commands, and Tauri interface
- `src-tauri/icons/`: application icons
- `src/App.test.tsx`: frontend regression tests

## Safety

Repository paths and frontend input are validated at the Tauri boundary. High-risk actions such as deleting files, discarding changes, and force-pushing retain confirmation flows; review the affected scope before proceeding. Configuration is loaded by schema version, and the previous valid file is backed up as `config.json.bak` before saving.
