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
- Show unified diffs; stage/unstage files and hunks; discard tracked files; move selected untracked files to macOS Trash.
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
- Refresh-all uses at most four Git processes and is tested with 50 repositories. The active repository receives priority.

## Interface

- Top workflows: Changes, History, Branches, Stashes. Branches contains tags, remotes, and submodules.
- Left: compact two-line repository dashboard. Center: one context canvas for unified diff, history graph, or comparison. Right: workflow lists and actions. Bottom: collapsible Git output.
- Changes uses Conflicts, Staged, Unstaged, and Untracked sections with explicit Stage/Unstage actions and a fixed commit composer.
- Advanced or dangerous actions live in contextual More menus. The output panel opens automatically only for failures or required attention.
- No repository file browser, editor tabs, side-by-side diff, syntax highlighting, command palette, CodeMirror, or internal three-way editor.

## Post-v1 backlog

The backlog is ordered by user risk and observed value. Do not start a lower tier while a higher-tier release blocker remains. Features without a stated trigger remain deferred.

### P1 — hardening before a public beta

Implementation status as of 2026-08-09:

- [x] Move clone into the asynchronous operation pipeline so it streams progress, supports cancellation, and reports or safely removes a partial destination after failure.
- [x] Strengthen process cancellation: send a graceful interrupt, wait briefly, then terminate the full process group; always refresh and report any Git operation state left behind.
- [ ] Replace the file watcher's leading-edge throttle with trailing debounce, suppress refresh storms caused by GitDock's own commands, and measure behavior in repositories containing large ignored dependency trees. The implementation and debounce regression test are complete; the large-tree measurement remains part of the clean-account smoke run.
- [x] Finish history pagination in the UI and retain the selected commit while additional pages load.
- [x] Replace remaining `window.prompt` and ordinary confirmation dialogs with validated in-app forms. Destructive Git operations must continue using the dedicated impact preview.
- [ ] Expand temporary-repository integration tests across every mutating `OperationRequest`, including hook failure, stale hunk rejection, linked-worktree locking, cancellation, merge/rebase/cherry-pick recovery, force-with-lease races, submodules, Trash, and configuration corruption. The named Git and configuration scenarios are covered except the macOS Trash integration, which remains in the clean-account smoke run; exhaustive per-variant coverage is still required.
- [x] Add frontend tests for workflow navigation, operation previews, output-panel failure behavior, conflict actions, and close-with-running-operation handling.
- [ ] Run and record the manual macOS smoke checklist on a clean user account: Gatekeeper bypass, Git discovery, Keychain/SSH agent, GPG pinentry, external diff/merge tools, linked worktrees, bare repositories, file permissions, app relaunch, and `.app` install/uninstall. The checklist is in `docs/MACOS_SMOKE.md` and has not been run.
- [x] Add versioned JSON migration tests and a recoverable backup path before the first configuration schema change.

Exit criteria: all automated checks pass, the smoke checklist has no data-loss or dead-end workflow, and clone/push/pull can be cancelled without orphaning child processes.

### P2 — usability and performance after beta feedback

- Improve the commit graph lane algorithm for wide merge histories, add incremental rendering, and benchmark repositories with at least 100,000 commits.
- Add side-by-side diff only if users repeatedly need it; keep unified diff as the default and reuse the existing snapshot/hunk safety model.
- Add syntax highlighting only if diff readability is a measured issue. Load language support on demand rather than bundling a full editor.
- Add a command palette only after the stable action set is large enough that menus measurably hurt discoverability.
- Add repository drag sorting and clearer group management if the current name/group controls are insufficient for users managing dozens of repositories.
- Add optional session log export with explicit user action and URL redaction; do not persist command output automatically.
- Add cached inactive-repository summaries only if refresh-all misses the 50-repository responsiveness target.
- Add signed and notarized Apple Silicon releases, release CI, checksums, and an update channel when the app is ready for external distribution.

### P3 — new capabilities, gated by demand

- Internal three-way conflict editor with base/current/incoming panes and per-block choices. Trigger: external tools are a recurring blocker in conflict workflows.
- Interactive rebase for reorder, reword, squash, fixup, and drop. Trigger: ordinary rebase plus cherry-pick no longer covers common history-editing needs.
- File history and blame. Trigger: users need repository investigation inside GitDock rather than their editor.
- Reflog browser and guarded recovery of lost commits/branches. Trigger: safe revert and last-commit undo prove insufficient.
- Arbitrary soft/mixed/hard reset with exact impact previews. Trigger: recovery UX and tests can demonstrate that staged and working-tree data cannot be silently lost.
- Remote tag deletion and signed tags. Trigger: release-management users request them and the confirmation model covers remote impact and GPG failure paths.
- GitHub/GitLab pull requests, issues, and CI status through provider APIs. Trigger: core local Git workflows are stable; credentials must use provider-supported OAuth/keychain storage.
- Windows and Linux builds. Trigger: macOS behavior is stable and platform-specific process cancellation, Trash, credential, path, watcher, and packaging tests are ready.
- Multi-repository batch writes. Trigger: a concrete workflow justifies them; every repository must receive an individual preview and independent result.
- Optional built-in AskPass prompts. Trigger: system credential helpers and SSH agents cause repeated onboarding failures; secrets must remain memory-only.

Still out of scope unless this plan is explicitly revised: custom Git protocols, credential storage, accounts, telemetry, embedded terminal, project build tools, general code editing, and arbitrary shell execution.
