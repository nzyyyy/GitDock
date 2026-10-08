# GitDock

[中文](README.md)

GitDock is a macOS Git desktop client built with Tauri, React, TypeScript, and Rust. It brings common Git workflows into a compact desktop interface and previews destructive operations before they run.

## Features

- The History sidebar search uses two rows: choose Commit message / SHA with segmented buttons above, then enter a query and click Search or press Enter below; clear the query inside the input. Search by complete commit message (case-insensitive literal matching) or a full / unique SHA prefix of at least four characters; message search covers the selected branch’s entire history with pagination, showing a result list until cleared to restore the graph. SHA searches check reachability for a selected branch and can inspect repository commit objects directly when all branches are selected.
- Open the repository directory in macOS Terminal, Finder, or an editor from the repository menu or command palette; choose an editor `.app` once, remember it globally, and change it at any time.
- The Branches list shows the tip commit’s committer date next to each local and remote branch SHA, displayed as YYYY/MM/DD in the system time zone; hover for the full timestamp in the interface language. Remote dates come from local remote-tracking refs without an automatic fetch.
- Search branch names in the menu next to the Commits heading to filter the commit list and graph by a local or remote branch, ranked by match quality, including its full ancestry and merged history without checking it out; all branches are shown by default.
- Branch comparisons use the commit-style file list with names, paths, line statistics, and a total count; open a file for a read-only diff and return to the list to select another, with support for additions, deletions, renames, binary files, and metadata-only changes; empty comparisons show an explicit message
- Inspect file history and blame with distinct loading, empty, and error states; switching repositories, workflows, or files discards stale inspection results
- Add, asynchronously clone, initialize, and manage local repositories; clone streams progress and can be cancelled
- Inspect working-tree status in a compact filename-and-path row, and review unified diffs (deleted lines have no number; added lines show the new-file number) with on-demand highlighting for common languages
- Stage, unstage, or discard files and individual change blocks and create commits; partially staged files show both staged and unstaged sides on the same diff page and can be batch-selected; resolve ordinary three-stage UTF-8 text conflicts block by block in Base / Current / Incoming panes, then stage the result
- Select multiple files to stage, unstage, discard, or trash them from one dropdown, including partially staged files; the checkbox to the left of Working Tree selects or clears all staged, partially staged, unstaged, and untracked files, including collapsed groups, and both the title and group checkboxes show an indeterminate state for partial selections; conflicted and ignored files are excluded
- Switch between English and Simplified Chinese with a remembered preference
- Smoothly scroll through a windowed commit topology graph whose lanes continue across pages (stashes and their internal helper commits stay in the Stashes pane); commits refresh the graph and list automatically, with commit details (metadata and the changed-file list), per-file diffs, cherry-pick, and revert actions
- Organize repositories with collapsible groups, a pinned Favorites group, new empty groups, and drag sorting, and rank repository search results by match quality within their groups; status rails and top-right counts identify working-tree changes on every registered repository as files change, not only the selected one, with keyboard ordering within a group
- Searching branches merges local and remote matches into one list ranked by match quality (exact, then prefix, then path-segment start, then plain substring), preferring local and current branches on ties while remote prefixes stay out of the match; clearing the search restores the local/remote groups with the current local branch pinned first and remote prefixes visually separated from branch paths; check out remote branches as local branches, then create, switch, merge, rebase, rename, and delete branches
- Use the trailing three-dot menu shown on hover for the same actions as the context menu on History commits and Branches, Tags, and Remotes entries
- Manage tags, remotes, stashes, and submodules, including complete tracked and untracked stash file lists and per-file diffs
- Fetch, pull, push, and force-push with lease; push creates the missing remote branch and sets upstream; split Pull and Push buttons show pending commit counts while their dropdowns group pull strategies and push settings; hover the in-progress spinner on Fetch, Pull, or Push and click stop to cancel; every Git operation shows a brief completion result
- Review affected paths and refs before sensitive Git operations run
- Enter Git operation details in validated in-app forms instead of browser prompts
- Explicitly export the bounded current-session Git log with URL credential redaction and no automatic persistence
- Use the `⌘K` / `Ctrl+K` command palette for stable workflows and repository actions, with commands ranked by match quality and driven from the keyboard; parameterized and dangerous operations retain their existing forms and impact previews
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
  - `src/lib/`: pure utilities (session-log ring buffer, search match ranking)
  - `src/types.ts`: shared types and constants; `src/api.ts`: Tauri command wrappers
  - `src/App.test.tsx`: frontend regression tests; pure-logic and component tests live next to their source
- `src-tauri/src/`: Rust backend, split by responsibility
  - `lib.rs`: `AppState` and Tauri command registration; `summary.rs`: repository summary refresh; `repositories.rs`: repository management and settings; `history.rs`: history and reference queries; `operations.rs`: Git operation engine and validation; `process.rs`: child processes, streams, and locks
  - `working_tree/`: working-tree snapshots, diffs, conflict caching, and stale-view validation; `repository_path.rs`: repository-relative path validation
  - `git.rs`: Git process adapter and remaining read queries; `models.rs`: shared data types; `store.rs`: persisted settings
- `src-tauri/icons/`: application icons

## Safety

Repository paths and frontend input are validated at the Tauri boundary. The internal conflict editor accepts only backend-owned block IDs and choices, then revalidates the snapshot, index stages, and working-tree contents before writing; unsupported conflicts continue through external merge tools. High-risk actions such as deleting files, discarding changes, and force-pushing retain confirmation flows; review the affected scope before proceeding. Configuration is loaded by schema version, and the previous valid file is backed up as `config.json.bak` before saving.

## License

This project is licensed under the [MIT](LICENSE) License.
