# GitDock

[中文](README.md)

GitDock is a macOS Git desktop client built with Tauri, React, TypeScript, and Rust. It brings common Git workflows into a compact desktop interface and previews destructive operations before they run.

## Features

- Add, clone, initialize, and manage local repositories
- Inspect working-tree status, file diffs, staged changes, and unstaged changes
- Stage or unstage files and hunks, create commits, and resolve conflicts
- Select multiple files to stage or unstage them in one operation
- Switch between English and Simplified Chinese with a remembered preference
- Browse the commit topology graph with branch and tag refs, inspect commit diffs, cherry-pick, and revert
- Browse local and remote branches in separate groups, then create, switch, merge, rebase, rename, and delete branches
- Manage tags, remotes, stashes, and submodules
- Fetch, pull, push, and force-push with lease
- Review affected paths and refs before sensitive Git operations run

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

Repository paths and frontend input are validated at the Tauri boundary. High-risk actions such as deleting files, discarding changes, and force-pushing retain confirmation flows; review the affected scope before proceeding.
