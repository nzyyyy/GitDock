# macOS clean-account smoke checklist

Release artifact: unsigned Apple Silicon `GitDock.app` (no DMG)

Record the macOS version, Git version, app version, commit, tester, and date before running this checklist. Use a clean macOS 14+ user account and mark every row Pass or Fail with notes; do not mark untested rows as passed.

| Area | Check | Status | Notes |
| --- | --- | --- | --- |
| Install | Copy `GitDock.app` to Applications, apply the documented Gatekeeper bypass, launch, quit, remove, and reinstall it | Not run | Requires clean user account |
| Git discovery | Discover system/Homebrew Git 2.30+ and validate a custom executable path | Not run | Requires packaged app |
| HTTPS auth | Fetch and push through the macOS Keychain credential helper without storing credentials in GitDock | Not run | Requires test remote |
| SSH auth | Fetch and push through the user's SSH agent | Not run | Requires test key and remote |
| Signing | Commit with GPG signing and complete pinentry | Not run | Requires test signing key |
| External tools | Open configured difftool and mergetool and return to GitDock | Not run | Requires configured tools |
| Worktrees | Register linked worktrees separately and verify writes serialize through their common Git directory | Not run | Requires test repository |
| Bare repository | Register a bare repository and verify all write actions remain unavailable | Not run | Requires test repository |
| Permissions | Open readable repositories and report inaccessible paths without changing permissions | Not run | Requires permission fixtures |
| Relaunch | Preserve repositories, selection, layout, language, and valid configuration after relaunch | Not run | Requires packaged app |
| Cancellation | Cancel clone, pull, and push; verify no descendant process survives and any remaining Git state is reported | Not run | Requires slow test remote |
| Partial clone | Cancel or fail clone and verify the partial destination is retained and reported | Not run | Requires slow or failing remote |

Release gate: every row must pass with no data-loss or dead-end workflow. Attach failures and reproduction steps to the release record.
