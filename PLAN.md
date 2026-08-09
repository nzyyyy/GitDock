# GitDock v1

GitDock is an English, dark, high-density Git desktop client for individual developers. The first test release targets macOS 14+ on Apple Silicon and ships as an unsigned `.app` without a DMG.

## Stack and boundaries

- Tauri 2, Rust, React, TypeScript, Vite, npm, and Node 24.
- Git 2.30+ is discovered from the app PATH, `/opt/homebrew/bin/git`, or `/usr/bin/git`, with a validated custom path override.
- Every Git invocation is an argument-array call owned by Rust. The frontend cannot execute arbitrary commands or submit arbitrary patches.
- GitDock uses the user's Git configuration, credential helpers, SSH agent, hooks, and GPG setup. It neither stores credentials nor implements Git protocols.
- One app window and process. Writes are serialized by common Git directory; reads and writes in unrelated repositories may run concurrently.
- Versioned JSON stores registered repositories and preferences. There is no database, account, telemetry, updater, plugin system, or embedded terminal.

## v1 capabilities

- Register, clone, initialize, remove, relocate, group, favorite, sort, and search repositories.
- Treat linked worktrees as separate entries with a shared write lock. Bare repositories are read-only.
- Show staged, unstaged, untracked, conflict, and on-demand ignored files. Watch only the active repository and debounce refreshes.
- Show unified or side-by-side diffs with on-demand syntax highlighting; stage/unstage files and hunks; discard tracked files; move selected untracked files to macOS Trash.
- Commit, amend, sign off, and run hooks. Show paginated history, commit diffs, refs, and a topology graph.
- Fetch, pull, push, set upstream, and manage remotes. Pull follows Git configuration. Force push is only `--force-with-lease` with an expected remote OID.
- Create, switch, rename, and delete branches; merge, squash merge, ordinary rebase, and cherry-pick.
- Resolve conflicts by choosing a whole side, opening the configured external tool, staging resolutions, and using the operation-specific continue/skip/abort actions.
- List/apply/pop/drop stashes; create lightweight/annotated tags; compare branches; manage direct submodules with optional explicit recursion.
- Revert one non-merge commit. Undo only an unpushed single-parent HEAD with soft reset.

## Safety and performance

- High-risk operations require a non-disableable preview that names the repository, paths/refs, affected commits, and recoverability.
- Hunk operations use backend-owned snapshot and hunk IDs. Stale snapshots fail and require refresh.
- Binary diffs show metadata only. Text diffs over 1 MiB or 20,000 lines defer to the user's difftool.
- Operation logs are bounded to the current session. Credential-like URL userinfo is redacted.
- Refresh-all synchronously refreshes the active repository, returns cached session summaries for inactive repositories, then revalidates them with at most four Git processes.

## Interface

- Top workflows: Changes, History, Branches, Stashes. Branches contains tags, remotes, and submodules.
- Left: compact two-line repository dashboard. Center: one context canvas for unified/side-by-side diff, history graph, or comparison. Right: workflow lists and actions. Bottom: collapsible Git output.
- Changes uses Conflicts, Staged, Unstaged, and Untracked sections with explicit Stage/Unstage actions and a fixed commit composer.
- Advanced or dangerous actions live in contextual More menus. The output panel opens automatically only for failures or required attention.
- A native command palette exposes stable repository/workflow actions through `⌘K`/`Ctrl+K`; dynamic row-item actions remain in context menus.
- No repository file browser, editor tabs, CodeMirror, or internal three-way editor.

## Post-v1 backlog

The backlog is ordered by user risk and observed value. Do not start a lower tier while a higher-tier release blocker remains. Features without a stated trigger remain deferred.

### P1 — hardening before a public beta

Implementation status as of 2026-08-09:

- [x] Move clone into the asynchronous operation pipeline so it streams progress, supports cancellation, and reports or safely removes a partial destination after failure.
- [x] Strengthen process cancellation: send a graceful interrupt, wait briefly, then terminate the full process group; always refresh and report any Git operation state left behind.
- [x] Replace the file watcher's leading-edge throttle with trailing debounce, suppress refresh storms caused by GitDock's own commands, and benchmark a repository containing a 100,000-file ignored dependency tree.
- [x] Prioritize the active repository during a 50-repository refresh and publish its completed summary after the first four-process batch, without waiting for the full refresh to finish.
- [x] Finish history pagination in the UI and retain the selected commit while additional pages load.
- [x] Replace remaining `window.prompt` and ordinary confirmation dialogs with validated in-app forms. Destructive Git operations must continue using the dedicated impact preview.
- [x] Expand temporary-repository integration tests across mutating `OperationRequest` groups, including real file/hunk/index/commit effects, recovery, hook failure, stale hunks, linked-worktree locking, cancellation, remotes and lease races, submodules, repository-local external tools, Trash injection, and configuration corruption. The 37 non-special command variants retain a command-spec completeness guard.
- [x] Add frontend tests for workflow navigation, operation previews, output-panel failure behavior, conflict actions, and close-with-running-operation handling.
- [x] Add versioned JSON migration tests and a recoverable backup path before the first configuration schema change.

Exit criteria: all automated checks pass, and clone/push/pull can be cancelled without orphaning child processes.

### P2 — usability and performance after beta feedback

- [x] Carry compact graph lanes in validated cursors across history pages, auto-load on scroll with a button fallback, and window both history panes. Graph edges use row buckets so scrolling does not rescan the full edge set; a 600-commit regression covers long cross-window edges. The 100,009-commit benchmark records 363 ms first-page p95 and 369 ms deep-page p95 in `docs/PERFORMANCE.md`.
- [x] Add side-by-side diff while keeping unified diff as the session default and reusing the existing snapshot/hunk safety model. The patch parser preserves real line numbers, multi-file boundaries, and no-newline markers, with raw-text fallback.
- [x] Add on-demand Highlight.js core/language loading for TypeScript/JavaScript, JSON, HTML/XML/SVG, CSS, Rust, Python, Shell, Markdown, YAML, and TOML; unknown extensions remain plain text.
- [x] Add a native-dialog command palette for stable workflow, repository, remote, language, and Git-selection actions. Parameterized and dangerous actions continue through existing forms and previews.
- [x] Add atomic repository drag sorting, collapsible groups, a pinned Favorites group, cross-group moves, and keyboard ordering controls. Failed persistence rolls back in-memory ordering, and drag/keyboard reordering is disabled while search filters the list.
- [x] Add ISO-timestamped session logs bounded to 10,000 entries or 5 MiB, plus explicit export through a backend-owned native save dialog. Export revalidates limits, redacts URL userinfo, and writes atomically; command output is never persisted automatically.
- [x] Add session-only inactive-repository summary caching with current metadata overlay, path/Git invalidation, generation guards, four-process background revalidation, and streamed summary events. The 50-entry cached response benchmark records 128 ms p95.

Current verification: 31 frontend tests and 37 Rust tests pass, along with the production frontend build and Rust formatting check. Three performance benchmarks remain ignored during normal test runs.

### P3 — new capabilities, gated by demand

- Internal three-way conflict editor with base/current/incoming panes and per-block choices. Trigger: external tools are a recurring blocker in conflict workflows.
- Interactive rebase for reorder, reword, squash, fixup, and drop. Trigger: ordinary rebase plus cherry-pick no longer covers common history-editing needs.
- File history and blame. Trigger: users need repository investigation inside GitDock rather than their editor.
- Reflog browser and guarded recovery of lost commits/branches. Trigger: safe revert and last-commit undo prove insufficient.
- Arbitrary soft/mixed/hard reset with exact impact previews. Trigger: recovery UX and tests can demonstrate that staged and working-tree data cannot be silently lost.
- Remote tag deletion and signed tags. Trigger: release-management users request them and the confirmation model covers remote impact and GPG failure paths.
- GitHub/GitLab pull requests, issues, and CI status through provider APIs. Trigger: core local Git workflows are stable; credentials must use provider-supported OAuth/keychain storage.
- Windows and Linux builds. Trigger: macOS behavior is stable and cross-platform support becomes a priority.
- Multi-repository batch writes. Trigger: a concrete workflow justifies them; every repository must receive an individual preview and independent result.
- Optional built-in AskPass prompts. Trigger: system credential helpers and SSH agents cause repeated onboarding failures; secrets must remain memory-only.
- Signed and notarized Apple Silicon releases, release CI, checksums, and an update channel. Trigger: repository hosting, Apple credentials, and the external distribution decision are ready.

Still out of scope unless this plan is explicitly revised: custom Git protocols, credential storage, accounts, telemetry, embedded terminal, project build tools, general code editing, and arbitrary shell execution.
