use crate::{
    git::{ensure_success, file_executable, render_conflict_resolution, strings, Git},
    models::*,
    process::{
        acquire_lock, emit, finish_operation, release_lock, resume_watch, spawn_git_operation,
        suppress_watch, terminate_process_group, OperationContext,
    },
    AppState,
};
use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    sync::atomic::Ordering,
    thread,
};
use tauri::{AppHandle, Manager, State};

#[tauri::command]
pub(crate) fn preview_operation(
    repository_id: RepositoryId,
    request: OperationRequest,
    state: State<'_, AppState>,
) -> Result<OperationPreview, String> {
    let record = state.record(repository_id)?;
    let mut result = preview(&record, &request)?;
    if let OperationRequest::ForcePushWithLease {
        expected_oid,
        branch,
        ..
    } = &request
    {
        let git = state.git()?;
        let cwd = Path::new(&record.path);
        verify_oid(&git, cwd, expected_oid)?;
        let commits = git
            .text(
                cwd,
                &["log", "--format=%h %s", &format!("{expected_oid}..HEAD")],
            )
            .unwrap_or_default();
        result.summary = format!(
            "Replace remote branch {branch} only while it still points to {}.\nCommits to publish:\n{}\nRepository: {}",
            &expected_oid[..expected_oid.len().min(12)],
            if commits.is_empty() { "(none)" } else { &commits },
            record.name
        );
    }
    if let OperationRequest::SubmoduleUpdate { paths, .. } = &request {
        if paths.is_empty() {
            result.affected_paths = state
                .git()?
                .submodules(Path::new(&record.path))?
                .into_iter()
                .map(|module| module.path)
                .collect();
        }
    }
    Ok(result)
}

#[tauri::command]
pub(crate) fn start_operation(
    repository_id: RepositoryId,
    request: OperationRequest,
    confirmed: bool,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<OperationResult, String> {
    let preview = preview_operation(repository_id, request.clone(), state.clone())?;
    if preview.requires_confirmation && !confirmed {
        return Err("This operation requires confirmation".into());
    }
    let git = state.git()?;
    let record = state.record(repository_id)?;
    let inspection = git.inspect_repository(Path::new(&record.path))?;
    if inspection.bare {
        return Err("Bare repositories are read-only in GitDock v1".into());
    }
    validate_request(&git, &inspection.root, &request)?;

    let operation_id = state.next_operation_id.fetch_add(1, Ordering::Relaxed);
    if let OperationRequest::ResolveConflictBlocks {
        snapshot_id,
        document_id,
        path,
        choices,
    } = request
    {
        acquire_lock(&state, &inspection.common_git_dir)?;
        suppress_watch(&state, repository_id);
        let root = inspection.root.clone();
        let common = inspection.common_git_dir.clone();
        thread::spawn(move || {
            emit(
                &app,
                operation_id,
                Some(repository_id),
                OperationEventKind::Started,
                "Resolve conflict blocks",
                None,
                None,
            );
            let result = resolve_conflict_blocks(
                &git,
                &root,
                &app.state::<AppState>(),
                repository_id,
                snapshot_id,
                &document_id,
                &path,
                &choices,
            );
            let (message, code, outcome) = match result {
                Ok(()) => (
                    "Conflict resolved and staged".to_string(),
                    0,
                    OperationOutcome::Succeeded,
                ),
                Err(error) => (error, 1, OperationOutcome::Failed),
            };
            if code != 0 {
                emit(
                    &app,
                    operation_id,
                    Some(repository_id),
                    OperationEventKind::Stderr,
                    &message,
                    None,
                    None,
                );
            }
            emit(
                &app,
                operation_id,
                Some(repository_id),
                OperationEventKind::Finished,
                &message,
                Some(code),
                Some(outcome),
            );
            finish_operation(&app, operation_id, repository_id, &common);
        });
        return Ok(OperationResult {
            operation_id,
            accepted: true,
        });
    }
    if let OperationRequest::TrashUntracked { paths } = request {
        acquire_lock(&state, &inspection.common_git_dir)?;
        suppress_watch(&state, repository_id);
        let root = inspection.root.clone();
        let common = inspection.common_git_dir.clone();
        thread::spawn(move || {
            emit(
                &app,
                operation_id,
                Some(repository_id),
                OperationEventKind::Started,
                "Move untracked files to Trash",
                None,
                None,
            );
            let result = trash_paths(&root, &paths, |path| {
                trash::delete(path).map_err(|error| error.to_string())
            });
            let (message, code) = match result {
                Ok(()) => ("Moved to Trash".to_string(), 0),
                Err(error) => (error, 1),
            };
            if code != 0 {
                emit(
                    &app,
                    operation_id,
                    Some(repository_id),
                    OperationEventKind::Stderr,
                    &message,
                    None,
                    None,
                );
            }
            emit(
                &app,
                operation_id,
                Some(repository_id),
                OperationEventKind::Finished,
                &message,
                Some(code),
                Some(if code == 0 {
                    OperationOutcome::Succeeded
                } else {
                    OperationOutcome::Failed
                }),
            );
            finish_operation(&app, operation_id, repository_id, &common);
        });
        return Ok(OperationResult {
            operation_id,
            accepted: true,
        });
    }

    let spec = command_spec(&state, repository_id, &inspection.root, &request)?;
    let cleanup_dir = spec.cleanup_dir.clone();
    if let Err(error) = acquire_lock(&state, &inspection.common_git_dir) {
        remove_dir(&cleanup_dir);
        return Err(error);
    }
    suppress_watch(&state, repository_id);
    if let Err(error) = spawn_git_operation(
        &git,
        &inspection.root,
        spec,
        operation_id,
        &preview.title,
        OperationContext::Repository {
            repository_id,
            common_git_dir: inspection.common_git_dir.clone(),
        },
        &state,
        app,
    ) {
        release_lock(&state, &inspection.common_git_dir);
        resume_watch(&state, repository_id);
        remove_dir(&cleanup_dir);
        return Err(error);
    }
    Ok(OperationResult {
        operation_id,
        accepted: true,
    })
}

fn trash_paths(
    root: &Path,
    paths: &[String],
    mut delete: impl FnMut(&Path) -> Result<(), String>,
) -> Result<(), String> {
    paths.iter().try_for_each(|path| {
        validate_relative_path(path)?;
        delete(&root.join(path))
    })
}

fn remove_dir(dir: &Option<PathBuf>) {
    if let Some(dir) = dir {
        let _ = fs::remove_dir_all(dir);
    }
}

fn resolve_conflict_blocks(
    git: &Git,
    root: &Path,
    state: &AppState,
    repository_id: RepositoryId,
    snapshot_id: u64,
    document_id: &str,
    path: &str,
    choices: &[ConflictResolution],
) -> Result<(), String> {
    validate_relative_path(path)?;
    let target = safe_worktree_file(root, path)?;
    let source = {
        let snapshots = state
            .snapshots
            .lock()
            .map_err(|_| "Snapshot cache is busy")?;
        let snapshot = snapshots
            .get(&snapshot_id)
            .ok_or("This conflict view is stale. Refresh and try again.")?;
        if snapshot.repository_id != repository_id {
            return Err("Snapshot does not belong to this repository".into());
        }
        let current_head = git.text(root, &["rev-parse", "--verify", "HEAD"]).ok();
        if snapshot.head_oid != current_head {
            return Err("HEAD changed. Refresh the repository and try again.".into());
        }
        snapshot
            .conflicts
            .get(document_id)
            .filter(|source| source.document.path == path)
            .cloned()
            .ok_or("Conflict document is not available in this snapshot")?
    };
    if git.conflict_stages(root, path)? != source.stages {
        return Err(
            "Conflict stages changed after this editor was opened. Refresh and try again.".into(),
        );
    }
    if fs::read(&target).map_err(|error| error.to_string())? != source.worktree {
        return Err(
            "The working-tree file changed after this editor was opened. Refresh and try again."
                .into(),
        );
    }
    if file_executable(&target)? != source.worktree_executable {
        return Err(
            "The working-tree file mode changed after this editor was opened. Refresh and try again."
                .into(),
        );
    }
    let result = render_conflict_resolution(&source.document.segments, choices)?;
    let permissions = fs::metadata(&target)
        .map_err(|error| error.to_string())?
        .permissions();
    replace_file(&target, &result, permissions.clone())?;
    let add = ensure_success(git.run(root, &with_paths(&["add"], &[path.into()]), None)?);
    if let Err(error) = add {
        if let Err(restore_error) = replace_file(&target, &source.worktree, permissions) {
            return Err(format!(
                "{error}; restoring the original file also failed: {restore_error}"
            ));
        }
        return Err(error);
    }
    Ok(())
}

pub(crate) fn safe_worktree_file(root: &Path, path: &str) -> Result<PathBuf, String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let target = root.join(path);
    let metadata = fs::symlink_metadata(&target).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_file() {
        return Err("Only regular files can use the internal conflict editor".into());
    }
    let parent = target
        .parent()
        .ok_or("Conflict path has no parent")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !parent.starts_with(&root) {
        return Err("Conflict path resolves outside the repository".into());
    }
    Ok(target)
}

fn replace_file(
    target: &Path,
    contents: &[u8],
    permissions: fs::Permissions,
) -> Result<(), String> {
    let parent = target.parent().ok_or("Conflict path has no parent")?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    temporary
        .write_all(contents)
        .map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .set_permissions(permissions)
        .map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    temporary
        .persist(target)
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub(crate) fn cancel_operation(
    operation_id: OperationId,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let (pid, cancelled) = state
        .running
        .lock()
        .map_err(|_| "Operation registry is busy")?
        .get(&operation_id)
        .map(|r| (r.pid, r.cancelled.clone()))
        .ok_or("Operation is not running")?;
    cancelled.store(true, Ordering::Relaxed);
    #[cfg(unix)]
    {
        terminate_process_group(pid)?;
    }
    Ok(())
}

fn preview(
    record: &RepositoryRecord,
    request: &OperationRequest,
) -> Result<OperationPreview, String> {
    let (title, summary, risk, paths, refs, recoverable) = match request {
        OperationRequest::DiscardTracked { paths } => (
            "Discard tracked changes",
            "Restore selected working-tree files from the index.",
            RiskLevel::Destructive,
            paths.clone(),
            vec![],
            false,
        ),
        OperationRequest::TrashUntracked { paths } => (
            "Move untracked files to Trash",
            "Move selected untracked paths to macOS Trash.",
            RiskLevel::Destructive,
            paths.clone(),
            vec![],
            true,
        ),
        OperationRequest::ResolveConflictBlocks { path, .. } => (
            "Resolve and stage conflict",
            "Replace the working-tree file with the selected conflict blocks, then stage it. Existing manual edits in the file will be overwritten.",
            RiskLevel::Destructive,
            vec![path.clone()],
            vec![],
            false,
        ),
        OperationRequest::DeleteBranch { name, force: true } => (
            "Force delete branch",
            "Delete the local branch even when it is not merged.",
            RiskLevel::Destructive,
            vec![],
            vec![name.clone()],
            false,
        ),
        OperationRequest::DeleteBranch { name, .. } => (
            "Delete branch",
            "Delete a merged local branch.",
            RiskLevel::Caution,
            vec![],
            vec![name.clone()],
            true,
        ),
        OperationRequest::ForcePushWithLease { remote, branch, .. } => (
            "Force push with lease",
            "Replace the remote branch only if its OID still matches the preview.",
            RiskLevel::Destructive,
            vec![],
            vec![format!("{remote}/{branch}")],
            false,
        ),
        OperationRequest::RemoveRemote { name } => (
            "Remove remote",
            "Remove this remote from repository configuration.",
            RiskLevel::Destructive,
            vec![],
            vec![name.clone()],
            true,
        ),
        OperationRequest::DeleteRemoteBranch { remote, branch } => (
            "Delete remote branch",
            "Delete this branch from the selected remote.",
            RiskLevel::Destructive,
            vec![],
            vec![format!("{remote}/{branch}")],
            false,
        ),
        OperationRequest::StashDrop { index } => (
            "Drop stash",
            "Permanently remove the selected stash entry.",
            RiskLevel::Destructive,
            vec![],
            vec![format!("stash@{{{index}}}")],
            false,
        ),
        OperationRequest::DeleteLocalTag { name } => (
            "Delete local tag",
            "Delete this tag from the local repository.",
            RiskLevel::Destructive,
            vec![],
            vec![name.clone()],
            true,
        ),
        OperationRequest::UndoLastCommit => (
            "Undo last commit",
            "Move HEAD to its first parent and keep the commit contents staged.",
            RiskLevel::Caution,
            vec![],
            vec!["HEAD".into()],
            true,
        ),
        OperationRequest::Revert { oid } => (
            "Revert commit",
            "Create a new commit that reverses the selected commit.",
            RiskLevel::Caution,
            vec![],
            vec![oid.clone()],
            true,
        ),
        OperationRequest::Abort { .. } => (
            "Abort operation",
            "Abort the in-progress Git operation.",
            RiskLevel::Caution,
            vec![],
            vec![],
            true,
        ),
        OperationRequest::SubmoduleUpdate { paths, recursive } => (
            "Update submodules",
            if *recursive {
                "Checkout recorded commits in selected submodules and nested submodules."
            } else {
                "Checkout recorded commits in selected direct submodules."
            },
            RiskLevel::Caution,
            paths.clone(),
            vec![],
            true,
        ),
        OperationRequest::InteractiveRebase { onto, .. } => (
            "Interactive rebase",
            "Rewrite the commits in HEAD that are not reachable from the selected base.",
            RiskLevel::Destructive,
            vec![],
            vec![format!("HEAD ({onto}..HEAD)")],
            false,
        ),
        _ => (
            operation_title(request),
            "Run the selected Git operation.",
            RiskLevel::Normal,
            request_paths(request),
            vec![],
            true,
        ),
    };
    Ok(OperationPreview {
        title: title.into(),
        summary: format!("{}\nRepository: {}", summary, record.name),
        risk,
        affected_paths: paths,
        affected_refs: refs,
        recoverable,
        requires_confirmation: risk >= RiskLevel::Caution,
    })
}

fn interactive_rebase_spec(
    onto: &str,
    plan: &[RebaseStep],
) -> Result<(Vec<String>, Vec<(String, String)>, PathBuf), String> {
    let mut todo = String::new();
    let mut events: Vec<Option<String>> = Vec::new();
    let mut seen_commit = false;
    for step in plan {
        if step.action == RebaseAction::Drop {
            continue;
        }
        if matches!(step.action, RebaseAction::Squash | RebaseAction::Fixup) && !seen_commit {
            return Err("A squash or fixup step cannot be the first commit".into());
        }
        seen_commit = true;
        match step.action {
            RebaseAction::Pick => {
                todo.push_str("pick ");
                todo.push_str(&step.oid);
                todo.push('\n');
            }
            RebaseAction::Reword => {
                let message = step
                    .message
                    .as_deref()
                    .ok_or("A reword step requires a message")?;
                if message.trim().is_empty() {
                    return Err("A reword step requires a message".into());
                }
                todo.push_str("reword ");
                todo.push_str(&step.oid);
                todo.push('\n');
                events.push(Some(message.to_string()));
            }
            RebaseAction::Squash => {
                todo.push_str("squash ");
                todo.push_str(&step.oid);
                todo.push('\n');
                events.push(None);
            }
            RebaseAction::Fixup => {
                todo.push_str("fixup ");
                todo.push_str(&step.oid);
                todo.push('\n');
            }
            RebaseAction::Drop => unreachable!(),
        }
    }
    if todo.is_empty() {
        return Err("Interactive rebase has no commits to apply".into());
    }

    let dir = tempfile::Builder::new()
        .prefix("gitdock-rebase-")
        .tempdir()
        .map_err(|error| error.to_string())?;

    fs::write(dir.path().join("todo"), todo.as_bytes()).map_err(|error| error.to_string())?;
    let seq_editor = dir.path().join("seq-editor.sh");
    fs::write(&seq_editor, "#!/bin/sh\ncp \"$GITDOCK_TMP/todo\" \"$1\"\n")
        .map_err(|error| error.to_string())?;
    set_executable(&seq_editor)?;

    let mut env = Vec::new();
    if events.iter().any(Option::is_some) {
        for (index, event) in events.iter().enumerate() {
            match event {
                Some(message) => {
                    fs::write(dir.path().join(format!("evt.{index}")), "reword")
                        .map_err(|error| error.to_string())?;
                    fs::write(dir.path().join(format!("msg.{index}")), message.as_bytes())
                        .map_err(|error| error.to_string())?;
                }
                None => {
                    fs::write(dir.path().join(format!("evt.{index}")), "keep")
                        .map_err(|error| error.to_string())?;
                }
            }
        }
        fs::write(dir.path().join("counter"), "0").map_err(|error| error.to_string())?;
        let msg_editor = dir.path().join("msg-editor.sh");
        fs::write(
            &msg_editor,
            "#!/bin/sh\nn=$(cat \"$GITDOCK_TMP/counter\")\nif [ \"$(cat \"$GITDOCK_TMP/evt.$n\")\" = reword ]; then\n  cat \"$GITDOCK_TMP/msg.$n\" > \"$1\"\nfi\necho $((n+1)) > \"$GITDOCK_TMP/counter\"\n",
        )
        .map_err(|error| error.to_string())?;
        set_executable(&msg_editor)?;
        env.push((
            "GIT_EDITOR".into(),
            msg_editor.to_string_lossy().into_owned(),
        ));
    }

    env.push((
        "GIT_SEQUENCE_EDITOR".into(),
        seq_editor.to_string_lossy().into_owned(),
    ));
    env.push((
        "GITDOCK_TMP".into(),
        dir.path().to_string_lossy().into_owned(),
    ));

    // Keep the TempDir alive: the spawned Git process needs the helper scripts.
    // The operation runner removes this directory after the process exits.
    let dir_path = dir.keep();
    Ok((
        vec!["rebase".into(), "-i".into(), onto.into()],
        env,
        dir_path,
    ))
}

fn set_executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)
            .map_err(|error| error.to_string())?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).map_err(|error| error.to_string())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

pub(crate) fn command_spec(
    state: &AppState,
    repository_id: RepositoryId,
    cwd: &Path,
    request: &OperationRequest,
) -> Result<crate::process::CommandSpec, String> {
    let mut input = None;
    let mut env = Vec::new();
    let mut cleanup_dir = None;
    let args: Vec<String> = match request {
        OperationRequest::StageFiles { paths } => with_paths(&["add"], paths),
        OperationRequest::UnstageFiles { paths } => with_paths(&["reset", "--quiet"], paths),
        OperationRequest::StageHunk {
            snapshot_id,
            hunk_id,
        } => {
            input = Some(cached_hunk(
                &state.git()?,
                cwd,
                state,
                repository_id,
                *snapshot_id,
                hunk_id,
            )?);
            strings(&["apply", "--cached", "--whitespace=nowarn", "-"])
        }
        OperationRequest::UnstageHunk {
            snapshot_id,
            hunk_id,
        } => {
            input = Some(cached_hunk(
                &state.git()?,
                cwd,
                state,
                repository_id,
                *snapshot_id,
                hunk_id,
            )?);
            strings(&["apply", "--cached", "--reverse", "--whitespace=nowarn", "-"])
        }
        OperationRequest::DiscardTracked { paths } => with_paths(&["restore", "--worktree"], paths),
        OperationRequest::Commit {
            message,
            amend,
            signoff,
        } => {
            if message.trim().is_empty() {
                return Err("Commit message is required".into());
            }
            input = Some(message.as_bytes().to_vec());
            let mut a = strings(&["commit", "-F", "-"]);
            if *amend {
                a.push("--amend".into());
            }
            if *signoff {
                a.push("--signoff".into());
            }
            a
        }
        OperationRequest::Fetch { remote, prune } => {
            let mut a = strings(&["fetch"]);
            if *prune {
                a.push("--prune".into());
            }
            if let Some(remote) = remote {
                a.push(remote.clone());
            }
            a
        }
        OperationRequest::Pull { strategy } => {
            let mut a = strings(&["pull"]);
            if let Some(strategy) = strategy {
                a.push(
                    match strategy {
                        PullStrategy::Merge => "--no-rebase",
                        PullStrategy::Rebase => "--rebase",
                        PullStrategy::FastForwardOnly => "--ff-only",
                    }
                    .into(),
                );
            }
            a
        }
        OperationRequest::Push { remote, branch } => {
            let mut a = strings(&["push"]);
            if let Some(remote) = remote {
                a.push(remote.clone());
            }
            if let Some(branch) = branch {
                a.push(branch.clone());
            }
            a
        }
        OperationRequest::ForcePushWithLease {
            remote,
            branch,
            expected_oid,
        } => vec![
            "push".into(),
            format!("--force-with-lease=refs/heads/{branch}:{expected_oid}"),
            remote.clone(),
            format!("HEAD:refs/heads/{branch}"),
        ],
        OperationRequest::SetUpstream { remote, branch } => {
            strings(&["branch", "--set-upstream-to", &format!("{remote}/{branch}")])
        }
        OperationRequest::AddRemote { name, url } => strings(&["remote", "add", name, url]),
        OperationRequest::SetRemoteUrl { name, url } => strings(&["remote", "set-url", name, url]),
        OperationRequest::RemoveRemote { name } => strings(&["remote", "remove", name]),
        OperationRequest::DeleteRemoteBranch { remote, branch } => {
            strings(&["push", remote, "--delete", branch])
        }
        OperationRequest::CreateBranch {
            name,
            start_point,
            checkout,
        } => {
            let mut a = strings(if *checkout {
                &["switch", "-c"][..]
            } else {
                &["branch"][..]
            });
            a.push(name.clone());
            if let Some(start) = start_point {
                a.push(start.clone());
            }
            a
        }
        OperationRequest::SwitchBranch { name } => strings(&["switch", name]),
        OperationRequest::RenameBranch { old_name, new_name } => {
            strings(&["branch", "-m", old_name, new_name])
        }
        OperationRequest::DeleteBranch { name, force } => {
            strings(&["branch", if *force { "-D" } else { "-d" }, name])
        }
        OperationRequest::Merge { reference, mode } => {
            let mut a = strings(&["merge", "--no-edit"]);
            a.push(
                match mode {
                    MergeMode::FastForward => "--ff-only",
                    MergeMode::Normal => "--no-ff",
                    MergeMode::Squash => "--squash",
                }
                .into(),
            );
            a.push(reference.clone());
            a
        }
        OperationRequest::Rebase { onto } => strings(&["rebase", onto]),
        OperationRequest::InteractiveRebase { onto, plan } => {
            let (args, spec_env, dir) = interactive_rebase_spec(onto, plan)?;
            env = spec_env;
            cleanup_dir = Some(dir);
            args
        }
        OperationRequest::CherryPick { commits } => {
            let mut a = strings(&["cherry-pick"]);
            a.extend(commits.clone());
            a
        }
        OperationRequest::Continue { kind } => continue_args(kind),
        OperationRequest::Skip { kind } => skip_args(kind)?,
        OperationRequest::Abort { kind } => abort_args(kind),
        OperationRequest::ChooseConflictSide { path, side } => with_paths(
            &[
                "checkout",
                match side {
                    ConflictSide::Ours => "--ours",
                    ConflictSide::Theirs => "--theirs",
                },
            ],
            std::slice::from_ref(path),
        ),
        OperationRequest::MarkResolved { paths } => with_paths(&["add"], paths),
        OperationRequest::StashCreate {
            message,
            include_untracked,
        } => {
            let mut a = strings(&["stash", "push"]);
            if *include_untracked {
                a.push("--include-untracked".into());
            }
            if let Some(message) = message {
                a.extend(["-m".into(), message.clone()]);
            }
            a
        }
        OperationRequest::StashApply { index, pop } => strings(&[
            "stash",
            if *pop { "pop" } else { "apply" },
            &format!("stash@{{{index}}}"),
        ]),
        OperationRequest::StashDrop { index } => {
            strings(&["stash", "drop", &format!("stash@{{{index}}}")])
        }
        OperationRequest::CreateTag {
            name,
            target,
            message,
        } => {
            let mut a = strings(&["tag"]);
            if let Some(message) = message {
                a.extend(["-a".into(), name.clone(), "-m".into(), message.clone()]);
            } else {
                a.push(name.clone());
            }
            if let Some(target) = target {
                a.push(target.clone());
            }
            a
        }
        OperationRequest::DeleteLocalTag { name } => strings(&["tag", "-d", name]),
        OperationRequest::PushTag { remote, name } => {
            strings(&["push", remote, &format!("refs/tags/{name}")])
        }
        OperationRequest::SubmoduleInit { paths, recursive } => {
            submodule_args("init", paths, *recursive)
        }
        OperationRequest::SubmoduleUpdate { paths, recursive } => {
            submodule_args("update", paths, *recursive)
        }
        OperationRequest::SubmoduleSync { paths, recursive } => {
            submodule_args("sync", paths, *recursive)
        }
        OperationRequest::Revert { oid } => strings(&["revert", "--no-edit", oid]),
        OperationRequest::UndoLastCommit => {
            validate_undo(&state.git()?, cwd)?;
            strings(&["reset", "--soft", "HEAD^"])
        }
        OperationRequest::RunDifftool { path, staged } => {
            let mut a = strings(&["difftool", "--no-prompt"]);
            if *staged {
                a.push("--cached".into());
            }
            a.extend(["--".into(), path.clone()]);
            a
        }
        OperationRequest::RunMergetool { path } => {
            let mut a = strings(&["mergetool", "--no-prompt"]);
            if let Some(path) = path {
                a.extend(["--".into(), path.clone()]);
            }
            a
        }
        OperationRequest::TrashUntracked { .. }
        | OperationRequest::ResolveConflictBlocks { .. } => unreachable!(),
    };
    Ok(crate::process::CommandSpec {
        args,
        input,
        env,
        cleanup_dir,
    })
}

fn validate_request(git: &Git, cwd: &Path, request: &OperationRequest) -> Result<(), String> {
    for path in request_paths(request) {
        validate_relative_path(&path)?;
    }
    match request {
        OperationRequest::TrashUntracked { paths } => {
            let status = git.status(0, cwd, false, 0)?;
            let untracked: HashSet<&str> = status
                .files
                .iter()
                .filter(|file| file.kind == ChangeKind::Untracked)
                .map(|file| file.path.as_str())
                .collect();
            if paths.iter().all(|path| untracked.contains(path.as_str())) {
                Ok(())
            } else {
                Err("Only paths that are currently untracked can be moved to Trash".into())
            }
        }
        OperationRequest::CreateBranch { name, .. }
        | OperationRequest::SwitchBranch { name }
        | OperationRequest::DeleteBranch { name, .. } => validate_branch(git, cwd, name),
        OperationRequest::RenameBranch { old_name, new_name } => {
            validate_branch(git, cwd, old_name)?;
            validate_branch(git, cwd, new_name)
        }
        OperationRequest::CreateTag { name, .. }
        | OperationRequest::DeleteLocalTag { name }
        | OperationRequest::PushTag { name, .. } => {
            validate_ref(git, cwd, &format!("refs/tags/{name}"))
        }
        OperationRequest::Revert { oid } => verify_commit(git, cwd, oid),
        OperationRequest::CherryPick { commits } => commits
            .iter()
            .try_for_each(|oid| verify_commit(git, cwd, oid)),
        OperationRequest::ForcePushWithLease { expected_oid, .. } => {
            verify_oid(git, cwd, expected_oid)
        }
        OperationRequest::InteractiveRebase { onto, plan } => {
            verify_commit(git, cwd, onto)?;
            let range = git.text(cwd, &["rev-list", "--topo-order", &format!("{onto}..HEAD")])?;
            let allowed: HashSet<&str> = range.split_whitespace().collect();
            let mut seen: HashSet<&str> = HashSet::new();
            for step in plan {
                verify_commit(git, cwd, &step.oid)?;
                if !seen.insert(step.oid.as_str()) {
                    return Err("The rebase plan lists a commit more than once".into());
                }
                if !allowed.contains(step.oid.as_str()) {
                    return Err(
                        "The rebase plan contains a commit outside the selected range".into(),
                    );
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

pub(crate) fn validate_relative_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || Path::new(path).components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        Err("Invalid repository-relative path".into())
    } else {
        Ok(())
    }
}

fn validate_branch(git: &Git, cwd: &Path, branch: &str) -> Result<(), String> {
    ensure_success(git.run(
        cwd,
        &strings(&["check-ref-format", "--branch", branch]),
        None,
    )?)
    .map(|_| ())
}
fn validate_ref(git: &Git, cwd: &Path, reference: &str) -> Result<(), String> {
    ensure_success(git.run(cwd, &strings(&["check-ref-format", reference]), None)?).map(|_| ())
}
pub(crate) fn verify_commit(git: &Git, cwd: &Path, oid: &str) -> Result<(), String> {
    ensure_success(git.run(
        cwd,
        &strings(&[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{oid}^{{commit}}"),
        ]),
        None,
    )?)
    .map(|_| ())
}
fn verify_oid(git: &Git, cwd: &Path, oid: &str) -> Result<(), String> {
    ensure_success(git.run(
        cwd,
        &strings(&["rev-parse", "--verify", "--end-of-options", oid]),
        None,
    )?)
    .map(|_| ())
}

fn validate_undo(git: &Git, cwd: &Path) -> Result<(), String> {
    let parents = git.text(cwd, &["rev-list", "--parents", "-n", "1", "HEAD"])?;
    if parents.split_whitespace().count() != 2 {
        return Err("Only a single-parent HEAD can be undone".into());
    }
    if git
        .run(
            cwd,
            &strings(&["merge-base", "--is-ancestor", "HEAD", "@{upstream}"]),
            None,
        )?
        .status
        .success()
    {
        return Err("HEAD is already reachable from its upstream. Use Revert instead.".into());
    }
    Ok(())
}

fn cached_hunk(
    git: &Git,
    cwd: &Path,
    state: &AppState,
    repository_id: RepositoryId,
    snapshot_id: u64,
    hunk_id: &str,
) -> Result<Vec<u8>, String> {
    let snapshots = state
        .snapshots
        .lock()
        .map_err(|_| "Snapshot cache is busy")?;
    let snapshot = snapshots
        .get(&snapshot_id)
        .ok_or("This diff is stale. Refresh and try again.")?;
    if snapshot.repository_id != repository_id {
        return Err("Snapshot does not belong to this repository".into());
    }
    let hunk = snapshot
        .hunks
        .get(hunk_id)
        .ok_or("Hunk is not available in this snapshot")?;
    let current = git.diff(cwd, &hunk.path, hunk.staged, snapshot_id)?;
    if current.patch != hunk.source_diff {
        return Err("This diff changed after it was displayed. Refresh and try again.".into());
    }
    Ok(hunk.patch.clone())
}

fn with_paths(prefix: &[&str], paths: &[String]) -> Vec<String> {
    let mut a = strings(prefix);
    a.push("--".into());
    a.extend(paths.iter().cloned());
    a
}
fn submodule_args(action: &str, paths: &[String], recursive: bool) -> Vec<String> {
    let mut a = strings(&["submodule", action]);
    if recursive {
        a.push("--recursive".into());
    }
    if !paths.is_empty() {
        a.push("--".into());
        a.extend(paths.iter().cloned());
    }
    a
}
fn continue_args(kind: &OngoingKind) -> Vec<String> {
    match kind {
        OngoingKind::Merge => strings(&["merge", "--continue"]),
        OngoingKind::Rebase => strings(&["rebase", "--continue"]),
        OngoingKind::CherryPick => strings(&["cherry-pick", "--continue"]),
        OngoingKind::Revert => strings(&["revert", "--continue"]),
    }
}
fn skip_args(kind: &OngoingKind) -> Result<Vec<String>, String> {
    match kind {
        OngoingKind::Rebase => Ok(strings(&["rebase", "--skip"])),
        OngoingKind::CherryPick => Ok(strings(&["cherry-pick", "--skip"])),
        OngoingKind::Revert => Ok(strings(&["revert", "--skip"])),
        OngoingKind::Merge => Err("Merge does not support skip".into()),
    }
}
fn abort_args(kind: &OngoingKind) -> Vec<String> {
    match kind {
        OngoingKind::Merge => strings(&["merge", "--abort"]),
        OngoingKind::Rebase => strings(&["rebase", "--abort"]),
        OngoingKind::CherryPick => strings(&["cherry-pick", "--abort"]),
        OngoingKind::Revert => strings(&["revert", "--abort"]),
    }
}

fn request_paths(request: &OperationRequest) -> Vec<String> {
    match request {
        OperationRequest::StageFiles { paths }
        | OperationRequest::UnstageFiles { paths }
        | OperationRequest::DiscardTracked { paths }
        | OperationRequest::TrashUntracked { paths }
        | OperationRequest::MarkResolved { paths }
        | OperationRequest::SubmoduleInit { paths, .. }
        | OperationRequest::SubmoduleUpdate { paths, .. }
        | OperationRequest::SubmoduleSync { paths, .. } => paths.clone(),
        OperationRequest::ChooseConflictSide { path, .. }
        | OperationRequest::RunDifftool { path, .. }
        | OperationRequest::ResolveConflictBlocks { path, .. } => vec![path.clone()],
        OperationRequest::RunMergetool { path } => path.clone().into_iter().collect(),
        _ => vec![],
    }
}

fn operation_title(request: &OperationRequest) -> &'static str {
    match request {
        OperationRequest::StageFiles { .. } | OperationRequest::StageHunk { .. } => "Stage changes",
        OperationRequest::UnstageFiles { .. } | OperationRequest::UnstageHunk { .. } => {
            "Unstage changes"
        }
        OperationRequest::Commit { amend: true, .. } => "Amend commit",
        OperationRequest::Commit { .. } => "Create commit",
        OperationRequest::Fetch { .. } => "Fetch",
        OperationRequest::Pull { .. } => "Pull",
        OperationRequest::Push { .. } => "Push",
        OperationRequest::CreateBranch { .. } => "Create branch",
        OperationRequest::SwitchBranch { .. } => "Switch branch",
        OperationRequest::Merge { .. } => "Merge branch",
        OperationRequest::Rebase { .. } => "Rebase branch",
        OperationRequest::CherryPick { .. } => "Cherry-pick commits",
        OperationRequest::StashCreate { .. } => "Create stash",
        OperationRequest::StashApply { pop: true, .. } => "Pop stash",
        OperationRequest::StashApply { .. } => "Apply stash",
        OperationRequest::CreateTag { .. } => "Create tag",
        OperationRequest::SubmoduleInit { .. } => "Initialize submodules",
        OperationRequest::SubmoduleSync { .. } => "Sync submodules",
        OperationRequest::RunDifftool { .. } => "Open difftool",
        OperationRequest::RunMergetool { .. } => "Open mergetool",
        OperationRequest::ResolveConflictBlocks { .. } => "Resolve and stage conflict",
        _ => "Run Git operation",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        git::{ensure_success, strings, Git},
        snapshot::{CachedHunk, SnapshotCache},
        test_util::{commit_file, git_ok, init_repo, run_request, test_state},
    };
    use std::collections::HashMap;

    #[test]
    fn destructive_actions_always_require_confirmation() {
        let record = RepositoryRecord {
            id: 1,
            path: "/tmp/repo".into(),
            name: "repo".into(),
            group: None,
            favorite: false,
            order: 0,
        };
        let discard_preview = preview(
            &record,
            &OperationRequest::DiscardTracked {
                paths: vec!["a".into()],
            },
        )
        .unwrap();
        assert_eq!(discard_preview.risk, RiskLevel::Destructive);
        assert!(discard_preview.requires_confirmation);
        let conflict = preview(
            &record,
            &OperationRequest::ResolveConflictBlocks {
                snapshot_id: 1,
                document_id: "document".into(),
                path: "file.txt".into(),
                choices: vec![],
            },
        )
        .unwrap();
        assert_eq!(conflict.risk, RiskLevel::Destructive);
        assert!(!conflict.recoverable);
        assert_eq!(conflict.affected_paths, ["file.txt"]);
    }

    #[test]
    fn command_specs_cover_every_non_special_operation_variant() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        ensure_success(git.run(dir.path(), &strings(&["init"]), None).unwrap()).unwrap();
        let state = test_state(git, dir.path().join("config.json"));
        let requests = vec![
            OperationRequest::StageFiles {
                paths: vec!["a".into()],
            },
            OperationRequest::UnstageFiles {
                paths: vec!["a".into()],
            },
            OperationRequest::DiscardTracked {
                paths: vec!["a".into()],
            },
            OperationRequest::Commit {
                message: "message".into(),
                amend: false,
                signoff: true,
            },
            OperationRequest::Fetch {
                remote: Some("origin".into()),
                prune: true,
            },
            OperationRequest::Pull {
                strategy: Some(PullStrategy::FastForwardOnly),
            },
            OperationRequest::Push {
                remote: Some("origin".into()),
                branch: Some("main".into()),
            },
            OperationRequest::ForcePushWithLease {
                remote: "origin".into(),
                branch: "main".into(),
                expected_oid: "a".repeat(40),
            },
            OperationRequest::SetUpstream {
                remote: "origin".into(),
                branch: "main".into(),
            },
            OperationRequest::AddRemote {
                name: "origin".into(),
                url: "https://example.test/repo".into(),
            },
            OperationRequest::SetRemoteUrl {
                name: "origin".into(),
                url: "https://example.test/next".into(),
            },
            OperationRequest::RemoveRemote {
                name: "origin".into(),
            },
            OperationRequest::DeleteRemoteBranch {
                remote: "origin".into(),
                branch: "old".into(),
            },
            OperationRequest::CreateBranch {
                name: "feature".into(),
                start_point: None,
                checkout: true,
            },
            OperationRequest::SwitchBranch {
                name: "main".into(),
            },
            OperationRequest::RenameBranch {
                old_name: "old".into(),
                new_name: "new".into(),
            },
            OperationRequest::DeleteBranch {
                name: "old".into(),
                force: false,
            },
            OperationRequest::Merge {
                reference: "feature".into(),
                mode: MergeMode::Squash,
            },
            OperationRequest::Rebase {
                onto: "main".into(),
            },
            OperationRequest::InteractiveRebase {
                onto: "main".into(),
                plan: vec![RebaseStep {
                    oid: "a".repeat(40),
                    action: RebaseAction::Pick,
                    message: None,
                }],
            },
            OperationRequest::CherryPick {
                commits: vec!["a".repeat(40)],
            },
            OperationRequest::Continue {
                kind: OngoingKind::Merge,
            },
            OperationRequest::Skip {
                kind: OngoingKind::Rebase,
            },
            OperationRequest::Abort {
                kind: OngoingKind::Revert,
            },
            OperationRequest::ChooseConflictSide {
                path: "a".into(),
                side: ConflictSide::Ours,
            },
            OperationRequest::MarkResolved {
                paths: vec!["a".into()],
            },
            OperationRequest::StashCreate {
                message: Some("save".into()),
                include_untracked: true,
            },
            OperationRequest::StashApply {
                index: 0,
                pop: false,
            },
            OperationRequest::StashDrop { index: 0 },
            OperationRequest::CreateTag {
                name: "v1".into(),
                target: None,
                message: Some("release".into()),
            },
            OperationRequest::DeleteLocalTag { name: "v1".into() },
            OperationRequest::PushTag {
                remote: "origin".into(),
                name: "v1".into(),
            },
            OperationRequest::SubmoduleInit {
                paths: vec!["module".into()],
                recursive: false,
            },
            OperationRequest::SubmoduleUpdate {
                paths: vec!["module".into()],
                recursive: true,
            },
            OperationRequest::SubmoduleSync {
                paths: vec!["module".into()],
                recursive: false,
            },
            OperationRequest::Revert {
                oid: "a".repeat(40),
            },
            OperationRequest::RunDifftool {
                path: "a".into(),
                staged: false,
            },
            OperationRequest::RunMergetool {
                path: Some("a".into()),
            },
        ];
        assert_eq!(requests.len(), 38);
        for request in requests {
            let spec = command_spec(&state, 1, dir.path(), &request).unwrap();
            assert!(!spec.args.is_empty(), "missing command for {request:?}");
        }
        for request in [
            OperationRequest::StageHunk {
                snapshot_id: 1,
                hunk_id: "missing".into(),
            },
            OperationRequest::UnstageHunk {
                snapshot_id: 1,
                hunk_id: "missing".into(),
            },
        ] {
            assert!(command_spec(&state, 1, dir.path(), &request)
                .err()
                .unwrap()
                .contains("stale"));
        }
        let record = RepositoryRecord {
            id: 1,
            path: dir.path().to_string_lossy().into(),
            name: "repo".into(),
            group: None,
            favorite: false,
            order: 0,
        };
        assert!(
            preview(
                &record,
                &OperationRequest::TrashUntracked {
                    paths: vec!["a".into()]
                }
            )
            .unwrap()
            .requires_confirmation
        );
    }

    #[test]
    fn rejects_paths_outside_repository() {
        assert!(validate_relative_path("../secret").is_err());
        assert!(validate_relative_path("/tmp/secret").is_err());
        assert!(validate_relative_path("src/main.rs").is_ok());
    }

    #[test]
    fn file_commit_stash_hunk_and_undo_requests_change_the_repository() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repo(&git, dir.path());
        commit_file(&git, dir.path(), "a.txt", "one\n", "base");
        let state = test_state(git.clone(), dir.path().join("config.json"));

        fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        run_request(
            &state,
            dir.path(),
            OperationRequest::StageFiles {
                paths: vec!["a.txt".into()],
            },
        );
        assert!(!git
            .text(dir.path(), &["diff", "--cached", "--name-only"])
            .unwrap()
            .is_empty());
        run_request(
            &state,
            dir.path(),
            OperationRequest::UnstageFiles {
                paths: vec!["a.txt".into()],
            },
        );
        assert!(git
            .text(dir.path(), &["diff", "--cached", "--name-only"])
            .unwrap()
            .is_empty());

        let diff = git.diff(dir.path(), "a.txt", false, 7).unwrap();
        let hunk = diff.hunks[0].clone();
        state.snapshots.lock().unwrap().insert(
            7,
            SnapshotCache {
                repository_id: 1,
                head_oid: Some(git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap()),
                hunks: HashMap::from([(
                    hunk.id.clone(),
                    CachedHunk {
                        path: "a.txt".into(),
                        staged: false,
                        patch: hunk.patch.into_bytes(),
                        source_diff: diff.patch,
                    },
                )]),
                conflicts: HashMap::new(),
            },
        );
        run_request(
            &state,
            dir.path(),
            OperationRequest::StageHunk {
                snapshot_id: 7,
                hunk_id: hunk.id.clone(),
            },
        );
        assert_eq!(git.text(dir.path(), &["show", ":a.txt"]).unwrap(), "two");
        let staged = git.diff(dir.path(), "a.txt", true, 8).unwrap();
        let staged_hunk = staged.hunks[0].clone();
        state.snapshots.lock().unwrap().insert(
            8,
            SnapshotCache {
                repository_id: 1,
                head_oid: Some(git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap()),
                hunks: HashMap::from([(
                    staged_hunk.id.clone(),
                    CachedHunk {
                        path: "a.txt".into(),
                        staged: true,
                        patch: staged_hunk.patch.into_bytes(),
                        source_diff: staged.patch,
                    },
                )]),
                conflicts: HashMap::new(),
            },
        );
        run_request(
            &state,
            dir.path(),
            OperationRequest::UnstageHunk {
                snapshot_id: 8,
                hunk_id: staged_hunk.id,
            },
        );
        assert_eq!(git.text(dir.path(), &["show", ":a.txt"]).unwrap(), "one");
        run_request(
            &state,
            dir.path(),
            OperationRequest::DiscardTracked {
                paths: vec!["a.txt".into()],
            },
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "one\n"
        );

        fs::write(dir.path().join("a.txt"), "committed\n").unwrap();
        run_request(
            &state,
            dir.path(),
            OperationRequest::StageFiles {
                paths: vec!["a.txt".into()],
            },
        );
        run_request(
            &state,
            dir.path(),
            OperationRequest::Commit {
                message: "signed".into(),
                amend: false,
                signoff: true,
            },
        );
        assert!(git
            .text(dir.path(), &["log", "-1", "--format=%B"])
            .unwrap()
            .contains("Signed-off-by:"));
        fs::write(dir.path().join("a.txt"), "amended\n").unwrap();
        run_request(
            &state,
            dir.path(),
            OperationRequest::StageFiles {
                paths: vec!["a.txt".into()],
            },
        );
        run_request(
            &state,
            dir.path(),
            OperationRequest::Commit {
                message: "amended".into(),
                amend: true,
                signoff: false,
            },
        );
        assert_eq!(
            git.text(dir.path(), &["log", "-1", "--format=%s"]).unwrap(),
            "amended"
        );

        fs::write(dir.path().join("a.txt"), "stashed\n").unwrap();
        fs::write(dir.path().join("new.txt"), "new\n").unwrap();
        run_request(
            &state,
            dir.path(),
            OperationRequest::StashCreate {
                message: Some("save".into()),
                include_untracked: true,
            },
        );
        assert!(!dir.path().join("new.txt").exists());
        run_request(
            &state,
            dir.path(),
            OperationRequest::StashApply {
                index: 0,
                pop: false,
            },
        );
        assert!(dir.path().join("new.txt").exists());
        git_ok(&git, dir.path(), &["reset", "--hard"]);
        git_ok(&git, dir.path(), &["clean", "-fd"]);
        run_request(&state, dir.path(), OperationRequest::StashDrop { index: 0 });
        assert!(git.text(dir.path(), &["stash", "list"]).unwrap().is_empty());

        run_request(&state, dir.path(), OperationRequest::UndoLastCommit);
        assert_eq!(
            git.text(dir.path(), &["log", "-1", "--format=%s"]).unwrap(),
            "base"
        );
        assert!(!git
            .text(dir.path(), &["diff", "--cached", "--name-only"])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn trash_uses_only_validated_repository_paths() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a"), "a").unwrap();
        fs::create_dir(dir.path().join("nested")).unwrap();
        fs::write(dir.path().join("nested/b"), "b").unwrap();
        let mut removed = Vec::new();
        trash_paths(dir.path(), &["a".into(), "nested/b".into()], |path| {
            removed.push(path.to_path_buf());
            fs::remove_file(path).map_err(|error| error.to_string())
        })
        .unwrap();
        assert_eq!(removed.len(), 2);
        assert!(trash_paths(dir.path(), &["../outside".into()], |_| Ok(())).is_err());
        assert!(trash_paths(dir.path(), &["missing".into()], |_| Err(
            "delete failed".into()
        ))
        .is_err());
    }

    #[test]
    fn branch_merge_rebase_cherry_pick_revert_and_recovery_requests_execute() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repo(&git, dir.path());
        git_ok(&git, dir.path(), &["config", "core.editor", "true"]);
        commit_file(&git, dir.path(), "base", "base\n", "base");
        let state = test_state(git.clone(), dir.path().join("config.json"));

        run_request(
            &state,
            dir.path(),
            OperationRequest::CreateBranch {
                name: "feature".into(),
                start_point: None,
                checkout: true,
            },
        );
        commit_file(&git, dir.path(), "feature", "feature\n", "feature");
        run_request(
            &state,
            dir.path(),
            OperationRequest::SwitchBranch {
                name: "main".into(),
            },
        );
        run_request(
            &state,
            dir.path(),
            OperationRequest::Merge {
                reference: "feature".into(),
                mode: MergeMode::FastForward,
            },
        );
        assert_eq!(
            git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap(),
            git.text(dir.path(), &["rev-parse", "feature"]).unwrap()
        );

        run_request(
            &state,
            dir.path(),
            OperationRequest::CreateBranch {
                name: "normal".into(),
                start_point: None,
                checkout: true,
            },
        );
        commit_file(&git, dir.path(), "normal", "normal\n", "normal");
        run_request(
            &state,
            dir.path(),
            OperationRequest::SwitchBranch {
                name: "main".into(),
            },
        );
        commit_file(&git, dir.path(), "main", "main\n", "main");
        run_request(
            &state,
            dir.path(),
            OperationRequest::Merge {
                reference: "normal".into(),
                mode: MergeMode::Normal,
            },
        );
        assert_eq!(
            git.text(dir.path(), &["rev-list", "--parents", "-n", "1", "HEAD"])
                .unwrap()
                .split_whitespace()
                .count(),
            3
        );

        run_request(
            &state,
            dir.path(),
            OperationRequest::CreateBranch {
                name: "squash".into(),
                start_point: None,
                checkout: true,
            },
        );
        commit_file(&git, dir.path(), "squash", "squash\n", "squash");
        run_request(
            &state,
            dir.path(),
            OperationRequest::SwitchBranch {
                name: "main".into(),
            },
        );
        run_request(
            &state,
            dir.path(),
            OperationRequest::Merge {
                reference: "squash".into(),
                mode: MergeMode::Squash,
            },
        );
        assert_eq!(
            git.text(dir.path(), &["diff", "--cached", "--name-only"])
                .unwrap(),
            "squash"
        );
        git_ok(&git, dir.path(), &["reset", "--hard"]);

        run_request(
            &state,
            dir.path(),
            OperationRequest::CreateBranch {
                name: "rebased".into(),
                start_point: None,
                checkout: true,
            },
        );
        commit_file(&git, dir.path(), "rebased", "rebased\n", "rebased");
        run_request(
            &state,
            dir.path(),
            OperationRequest::SwitchBranch {
                name: "main".into(),
            },
        );
        commit_file(&git, dir.path(), "main-2", "main-2\n", "main-2");
        run_request(
            &state,
            dir.path(),
            OperationRequest::SwitchBranch {
                name: "rebased".into(),
            },
        );
        run_request(
            &state,
            dir.path(),
            OperationRequest::Rebase {
                onto: "main".into(),
            },
        );
        assert!(git
            .run(
                dir.path(),
                &strings(&["merge-base", "--is-ancestor", "main", "HEAD"]),
                None
            )
            .unwrap()
            .status
            .success());

        run_request(
            &state,
            dir.path(),
            OperationRequest::CreateBranch {
                name: "pick".into(),
                start_point: Some("main".into()),
                checkout: true,
            },
        );
        commit_file(&git, dir.path(), "picked", "picked\n", "picked");
        let picked = git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        run_request(
            &state,
            dir.path(),
            OperationRequest::SwitchBranch {
                name: "main".into(),
            },
        );
        run_request(
            &state,
            dir.path(),
            OperationRequest::CherryPick {
                commits: vec![picked.clone()],
            },
        );
        assert!(dir.path().join("picked").exists());
        run_request(&state, dir.path(), OperationRequest::Revert { oid: picked });
        assert!(!dir.path().join("picked").exists());
        run_request(
            &state,
            dir.path(),
            OperationRequest::RenameBranch {
                old_name: "pick".into(),
                new_name: "picked-branch".into(),
            },
        );
        run_request(
            &state,
            dir.path(),
            OperationRequest::DeleteBranch {
                name: "picked-branch".into(),
                force: true,
            },
        );

        fs::write(dir.path().join("base"), "main conflict\n").unwrap();
        git_ok(&git, dir.path(), &["add", "base"]);
        git_ok(&git, dir.path(), &["commit", "-m", "main conflict"]);
        run_request(
            &state,
            dir.path(),
            OperationRequest::CreateBranch {
                name: "conflict".into(),
                start_point: Some("HEAD^".into()),
                checkout: true,
            },
        );
        fs::write(dir.path().join("base"), "branch conflict\n").unwrap();
        git_ok(&git, dir.path(), &["add", "base"]);
        git_ok(&git, dir.path(), &["commit", "-m", "branch conflict"]);
        run_request(
            &state,
            dir.path(),
            OperationRequest::SwitchBranch {
                name: "main".into(),
            },
        );
        let spec = command_spec(
            &state,
            1,
            dir.path(),
            &OperationRequest::Merge {
                reference: "conflict".into(),
                mode: MergeMode::Normal,
            },
        )
        .unwrap();
        assert!(!git
            .run(dir.path(), &spec.args, spec.input.as_deref())
            .unwrap()
            .status
            .success());
        run_request(
            &state,
            dir.path(),
            OperationRequest::ChooseConflictSide {
                path: "base".into(),
                side: ConflictSide::Ours,
            },
        );
        run_request(
            &state,
            dir.path(),
            OperationRequest::MarkResolved {
                paths: vec!["base".into()],
            },
        );
        run_request(
            &state,
            dir.path(),
            OperationRequest::Continue {
                kind: OngoingKind::Merge,
            },
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("base")).unwrap(),
            "main conflict\n"
        );
    }

    #[test]
    fn resolves_cached_conflict_blocks_and_stages_the_file() {
        for (choice, expected) in [
            (ConflictChoice::Current, "start\ncurrent\nend\n"),
            (ConflictChoice::Incoming, "start\nincoming\nend\n"),
            (ConflictChoice::Both, "start\ncurrent\nincoming\nend\n"),
        ] {
            let git = Git::discover(None).unwrap();
            let dir = tempfile::tempdir().unwrap();
            init_repo(&git, dir.path());
            commit_file(&git, dir.path(), "file.txt", "start\nbase\nend\n", "base");
            git_ok(&git, dir.path(), &["switch", "-c", "incoming"]);
            commit_file(
                &git,
                dir.path(),
                "file.txt",
                "start\nincoming\nend\n",
                "incoming",
            );
            git_ok(&git, dir.path(), &["switch", "main"]);
            commit_file(
                &git,
                dir.path(),
                "file.txt",
                "start\ncurrent\nend\n",
                "current",
            );
            assert!(!git
                .run(dir.path(), &strings(&["merge", "incoming"]), None)
                .unwrap()
                .status
                .success());

            let source = git.conflict_source(dir.path(), "file.txt", 7).unwrap();
            let block_id = source
                .document
                .segments
                .iter()
                .find_map(|segment| match segment {
                    ConflictSegment::Conflict { id, .. } => Some(id.clone()),
                    ConflictSegment::Context { .. } => None,
                })
                .unwrap();
            let document_id = source.document.id.clone();
            let state = test_state(git.clone(), dir.path().join("config.json"));
            state.snapshots.lock().unwrap().insert(
                7,
                SnapshotCache {
                    repository_id: 1,
                    head_oid: Some(git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap()),
                    hunks: HashMap::new(),
                    conflicts: HashMap::from([(document_id.clone(), source)]),
                },
            );
            resolve_conflict_blocks(
                &git,
                dir.path(),
                &state,
                1,
                7,
                &document_id,
                "file.txt",
                &[ConflictResolution { block_id, choice }],
            )
            .unwrap();
            assert_eq!(
                fs::read_to_string(dir.path().join("file.txt")).unwrap(),
                expected
            );
            assert!(git
                .text(dir.path(), &["ls-files", "--unmerged"])
                .unwrap()
                .is_empty());
            assert!(git
                .text(dir.path(), &["ls-files", "--stage", "--", "file.txt"])
                .unwrap()
                .contains(" 0\tfile.txt"));
        }
    }

    #[test]
    fn stale_conflict_editor_never_overwrites_external_changes() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repo(&git, dir.path());
        commit_file(&git, dir.path(), "file.txt", "base\n", "base");
        git_ok(&git, dir.path(), &["switch", "-c", "incoming"]);
        commit_file(&git, dir.path(), "file.txt", "incoming\n", "incoming");
        git_ok(&git, dir.path(), &["switch", "main"]);
        commit_file(&git, dir.path(), "file.txt", "current\n", "current");
        assert!(!git
            .run(dir.path(), &strings(&["merge", "incoming"]), None)
            .unwrap()
            .status
            .success());
        let source = git.conflict_source(dir.path(), "file.txt", 8).unwrap();
        let document_id = source.document.id.clone();
        let original_worktree = source.worktree.clone();
        let block_id = source
            .document
            .segments
            .iter()
            .find_map(|segment| match segment {
                ConflictSegment::Conflict { id, .. } => Some(id.clone()),
                ConflictSegment::Context { .. } => None,
            })
            .unwrap();
        let state = test_state(git.clone(), dir.path().join("config.json"));
        state.snapshots.lock().unwrap().insert(
            8,
            SnapshotCache {
                repository_id: 1,
                head_oid: Some(git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap()),
                hunks: HashMap::new(),
                conflicts: HashMap::from([(document_id.clone(), source)]),
            },
        );
        fs::write(dir.path().join("file.txt"), "external edit\n").unwrap();
        let refreshed = git.conflict_source(dir.path(), "file.txt", 8).unwrap();
        assert_ne!(document_id, refreshed.document.id);
        let error = resolve_conflict_blocks(
            &git,
            dir.path(),
            &state,
            1,
            8,
            &document_id,
            "file.txt",
            &[ConflictResolution {
                block_id: block_id.clone(),
                choice: ConflictChoice::Current,
            }],
        )
        .unwrap_err();
        assert!(error.contains("working-tree file changed"));
        assert_eq!(
            fs::read_to_string(dir.path().join("file.txt")).unwrap(),
            "external edit\n"
        );

        fs::write(dir.path().join("file.txt"), original_worktree).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let target = dir.path().join("file.txt");
            let mut permissions = fs::metadata(&target).unwrap().permissions();
            permissions.set_mode(permissions.mode() ^ 0o100);
            fs::set_permissions(&target, permissions).unwrap();
            let error = resolve_conflict_blocks(
                &git,
                dir.path(),
                &state,
                1,
                8,
                &document_id,
                "file.txt",
                &[ConflictResolution {
                    block_id,
                    choice: ConflictChoice::Current,
                }],
            )
            .unwrap_err();
            assert!(error.contains("file mode changed"));
        }
    }

    #[test]
    fn skip_and_abort_requests_clear_a_cherry_pick_conflict() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repo(&git, dir.path());
        commit_file(&git, dir.path(), "conflict", "base\n", "base");
        let base = git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        let state = test_state(git.clone(), dir.path().join("config.json"));

        run_request(
            &state,
            dir.path(),
            OperationRequest::CreateBranch {
                name: "source".into(),
                start_point: None,
                checkout: true,
            },
        );
        commit_file(&git, dir.path(), "conflict", "source\n", "source");
        let source = git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        run_request(
            &state,
            dir.path(),
            OperationRequest::SwitchBranch {
                name: "main".into(),
            },
        );
        commit_file(&git, dir.path(), "conflict", "main\n", "main");
        let main = git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        let spec = command_spec(
            &state,
            1,
            dir.path(),
            &OperationRequest::CherryPick {
                commits: vec![source],
            },
        )
        .unwrap();
        assert!(!git
            .run(dir.path(), &spec.args, spec.input.as_deref())
            .unwrap()
            .status
            .success());
        run_request(
            &state,
            dir.path(),
            OperationRequest::Skip {
                kind: OngoingKind::CherryPick,
            },
        );
        assert_eq!(git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap(), main);

        run_request(
            &state,
            dir.path(),
            OperationRequest::CreateBranch {
                name: "abort-source".into(),
                start_point: Some(base),
                checkout: true,
            },
        );
        commit_file(&git, dir.path(), "conflict", "abort\n", "abort source");
        let abort_source = git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        run_request(
            &state,
            dir.path(),
            OperationRequest::SwitchBranch {
                name: "main".into(),
            },
        );
        let spec = command_spec(
            &state,
            1,
            dir.path(),
            &OperationRequest::CherryPick {
                commits: vec![abort_source],
            },
        )
        .unwrap();
        assert!(!git
            .run(dir.path(), &spec.args, spec.input.as_deref())
            .unwrap()
            .status
            .success());
        run_request(
            &state,
            dir.path(),
            OperationRequest::Abort {
                kind: OngoingKind::CherryPick,
            },
        );
        assert_eq!(git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap(), main);
    }

    #[test]
    fn remote_pull_push_lease_branch_and_tag_requests_execute() {
        let git = Git::discover(None).unwrap();
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        let peer = root.path().join("peer");
        let remote = root.path().join("remote.git");
        fs::create_dir(&repo).unwrap();
        fs::create_dir(&remote).unwrap();
        init_repo(&git, &repo);
        git_ok(&git, &remote, &["init", "--bare"]);
        commit_file(&git, &repo, "a", "a\n", "base");
        let state = test_state(git.clone(), root.path().join("config.json"));
        let remote_url = remote.to_string_lossy().to_string();

        run_request(
            &state,
            &repo,
            OperationRequest::AddRemote {
                name: "origin".into(),
                url: remote_url.clone(),
            },
        );
        run_request(
            &state,
            &repo,
            OperationRequest::Push {
                remote: Some("origin".into()),
                branch: Some("main".into()),
            },
        );
        run_request(
            &state,
            &repo,
            OperationRequest::Fetch {
                remote: Some("origin".into()),
                prune: true,
            },
        );
        run_request(
            &state,
            &repo,
            OperationRequest::SetUpstream {
                remote: "origin".into(),
                branch: "main".into(),
            },
        );
        for strategy in [
            PullStrategy::Merge,
            PullStrategy::Rebase,
            PullStrategy::FastForwardOnly,
        ] {
            run_request(
                &state,
                &repo,
                OperationRequest::Pull {
                    strategy: Some(strategy),
                },
            );
        }

        let expected = git.text(&repo, &["rev-parse", "origin/main"]).unwrap();
        commit_file(&git, &repo, "a", "local\n", "local");
        run_request(
            &state,
            &repo,
            OperationRequest::ForcePushWithLease {
                remote: "origin".into(),
                branch: "main".into(),
                expected_oid: expected,
            },
        );
        let pushed = git.text(&repo, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(git.text(&remote, &["rev-parse", "main"]).unwrap(), pushed);

        git_ok(
            &git,
            root.path(),
            &["clone", "-b", "main", &remote_url, peer.to_str().unwrap()],
        );
        git_ok(&git, &peer, &["config", "user.name", "GitDock Tests"]);
        git_ok(
            &git,
            &peer,
            &["config", "user.email", "gitdock@example.com"],
        );
        commit_file(&git, &peer, "peer", "peer\n", "peer");
        git_ok(&git, &peer, &["push", "origin", "main"]);
        commit_file(&git, &repo, "local", "local\n", "local again");
        let spec = command_spec(
            &state,
            1,
            &repo,
            &OperationRequest::ForcePushWithLease {
                remote: "origin".into(),
                branch: "main".into(),
                expected_oid: pushed,
            },
        )
        .unwrap();
        assert!(!git
            .run(&repo, &spec.args, spec.input.as_deref())
            .unwrap()
            .status
            .success());

        run_request(
            &state,
            &repo,
            OperationRequest::CreateBranch {
                name: "remote-delete".into(),
                start_point: None,
                checkout: false,
            },
        );
        run_request(
            &state,
            &repo,
            OperationRequest::Push {
                remote: Some("origin".into()),
                branch: Some("remote-delete".into()),
            },
        );
        run_request(
            &state,
            &repo,
            OperationRequest::DeleteRemoteBranch {
                remote: "origin".into(),
                branch: "remote-delete".into(),
            },
        );
        assert!(git
            .text(
                &remote,
                &["show-ref", "--verify", "refs/heads/remote-delete"]
            )
            .is_err());

        run_request(
            &state,
            &repo,
            OperationRequest::CreateTag {
                name: "v1".into(),
                target: Some("HEAD".into()),
                message: Some("release".into()),
            },
        );
        run_request(
            &state,
            &repo,
            OperationRequest::PushTag {
                remote: "origin".into(),
                name: "v1".into(),
            },
        );
        assert!(git
            .text(&remote, &["show-ref", "--verify", "refs/tags/v1"])
            .is_ok());
        run_request(
            &state,
            &repo,
            OperationRequest::DeleteLocalTag { name: "v1".into() },
        );
        assert!(git
            .text(&repo, &["show-ref", "--verify", "refs/tags/v1"])
            .is_err());

        run_request(
            &state,
            &repo,
            OperationRequest::SetRemoteUrl {
                name: "origin".into(),
                url: remote_url.clone(),
            },
        );
        assert_eq!(
            git.text(&repo, &["remote", "get-url", "origin"]).unwrap(),
            remote_url
        );
        run_request(
            &state,
            &repo,
            OperationRequest::RemoveRemote {
                name: "origin".into(),
            },
        );
        assert!(git.text(&repo, &["remote"]).unwrap().is_empty());
    }

    #[test]
    fn submodule_and_repository_local_tool_requests_execute() {
        let git = Git::discover(None).unwrap();
        let root = tempfile::tempdir().unwrap();
        let module = root.path().join("module");
        let host = root.path().join("host");
        fs::create_dir(&module).unwrap();
        fs::create_dir(&host).unwrap();
        init_repo(&git, &module);
        commit_file(&git, &module, "module.txt", "module\n", "module");
        init_repo(&git, &host);
        commit_file(&git, &host, "tracked.txt", "base\n", "base");
        git_ok(
            &git,
            &host,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                module.to_str().unwrap(),
                "modules/one",
            ],
        );
        git_ok(&git, &host, &["commit", "-am", "submodule"]);
        git_ok(
            &git,
            &host,
            &["submodule", "deinit", "-f", "--", "modules/one"],
        );
        let clone = host;
        let state = test_state(git.clone(), root.path().join("config.json"));

        run_request(
            &state,
            &clone,
            OperationRequest::SubmoduleInit {
                paths: vec!["modules/one".into()],
                recursive: false,
            },
        );
        run_request(
            &state,
            &clone,
            OperationRequest::SubmoduleUpdate {
                paths: vec!["modules/one".into()],
                recursive: true,
            },
        );
        assert!(clone.join("modules/one/module.txt").exists());
        run_request(
            &state,
            &clone,
            OperationRequest::SubmoduleSync {
                paths: vec!["modules/one".into()],
                recursive: false,
            },
        );

        fs::write(clone.join("tracked.txt"), "changed\n").unwrap();
        git_ok(&git, &clone, &["config", "diff.tool", "gitdock-test"]);
        git_ok(
            &git,
            &clone,
            &[
                "config",
                "difftool.gitdock-test.cmd",
                "printf called > \"$MERGED.gd-difftool\"",
            ],
        );
        run_request(
            &state,
            &clone,
            OperationRequest::RunDifftool {
                path: "tracked.txt".into(),
                staged: false,
            },
        );
        assert_eq!(
            fs::read_to_string(clone.join("tracked.txt.gd-difftool")).unwrap(),
            "called"
        );

        git_ok(&git, &clone, &["restore", "tracked.txt"]);
        git_ok(&git, &clone, &["switch", "-c", "tool-conflict"]);
        commit_file(&git, &clone, "tracked.txt", "branch\n", "branch");
        git_ok(&git, &clone, &["switch", "main"]);
        commit_file(&git, &clone, "tracked.txt", "main\n", "main");
        assert!(!git
            .run(&clone, &strings(&["merge", "tool-conflict"]), None)
            .unwrap()
            .status
            .success());
        git_ok(&git, &clone, &["config", "merge.tool", "gitdock-test"]);
        git_ok(
            &git,
            &clone,
            &[
                "config",
                "mergetool.gitdock-test.cmd",
                "printf called > \"$MERGED.gd-mergetool\"",
            ],
        );
        git_ok(
            &git,
            &clone,
            &["config", "mergetool.gitdock-test.trustExitCode", "true"],
        );
        run_request(
            &state,
            &clone,
            OperationRequest::RunMergetool {
                path: Some("tracked.txt".into()),
            },
        );
        assert_eq!(
            fs::read_to_string(clone.join("tracked.txt.gd-mergetool")).unwrap(),
            "called"
        );
    }

    #[test]
    fn rejects_a_hunk_when_the_displayed_diff_changed() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        ensure_success(git.run(dir.path(), &strings(&["init"]), None).unwrap()).unwrap();
        for (key, value) in [
            ("user.name", "GitDock Test"),
            ("user.email", "test@gitdock.local"),
        ] {
            ensure_success(
                git.run(dir.path(), &strings(&["config", key, value]), None)
                    .unwrap(),
            )
            .unwrap();
        }
        fs::write(dir.path().join("file.txt"), "one\n").unwrap();
        ensure_success(
            git.run(dir.path(), &strings(&["add", "file.txt"]), None)
                .unwrap(),
        )
        .unwrap();
        ensure_success(
            git.run(dir.path(), &strings(&["commit", "-m", "initial"]), None)
                .unwrap(),
        )
        .unwrap();
        fs::write(dir.path().join("file.txt"), "two\n").unwrap();
        let state = test_state(git, dir.path().join("config.json"));
        state.snapshots.lock().unwrap().insert(
            1,
            SnapshotCache {
                repository_id: 1,
                head_oid: None,
                hunks: HashMap::from([(
                    "hunk".into(),
                    CachedHunk {
                        path: "file.txt".into(),
                        staged: false,
                        patch: b"invalid".to_vec(),
                        source_diff: "previous diff".into(),
                    },
                )]),
                conflicts: HashMap::new(),
            },
        );
        let error = command_spec(
            &state,
            1,
            dir.path(),
            &OperationRequest::StageHunk {
                snapshot_id: 1,
                hunk_id: "hunk".into(),
            },
        )
        .err()
        .unwrap();
        assert!(error.contains("changed after it was displayed"));
    }

    #[test]
    fn interactive_rebase_reorders_rewords_and_fixups() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repo(&git, dir.path());
        commit_file(&git, dir.path(), "base", "base\n", "base");
        let base = git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        commit_file(&git, dir.path(), "a", "a\n", "A");
        let a = git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        commit_file(&git, dir.path(), "b", "b\n", "B");
        let b = git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        commit_file(&git, dir.path(), "c", "c\n", "C");
        let c = git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        let state = test_state(git.clone(), dir.path().join("config.json"));

        run_request(
            &state,
            dir.path(),
            OperationRequest::InteractiveRebase {
                onto: base,
                plan: vec![
                    RebaseStep {
                        oid: c,
                        action: RebaseAction::Reword,
                        message: Some("C rewritten".into()),
                    },
                    RebaseStep {
                        oid: a,
                        action: RebaseAction::Pick,
                        message: None,
                    },
                    RebaseStep {
                        oid: b,
                        action: RebaseAction::Fixup,
                        message: None,
                    },
                ],
            },
        );
        let subjects = git.text(dir.path(), &["log", "--format=%s"]).unwrap();
        assert_eq!(subjects, "A\nC rewritten\nbase");
        assert!(dir.path().join("a").exists());
        assert!(dir.path().join("b").exists());
        assert!(dir.path().join("c").exists());
    }

    #[test]
    fn interactive_rebase_squashes_and_drops() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repo(&git, dir.path());
        commit_file(&git, dir.path(), "base", "base\n", "base");
        let base = git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        commit_file(&git, dir.path(), "a", "a\n", "A");
        let a = git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        commit_file(&git, dir.path(), "b", "b\n", "B");
        let b = git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        commit_file(&git, dir.path(), "c", "c\n", "C");
        let c = git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        let state = test_state(git.clone(), dir.path().join("config.json"));

        run_request(
            &state,
            dir.path(),
            OperationRequest::InteractiveRebase {
                onto: base,
                plan: vec![
                    RebaseStep {
                        oid: a,
                        action: RebaseAction::Pick,
                        message: None,
                    },
                    RebaseStep {
                        oid: c,
                        action: RebaseAction::Squash,
                        message: None,
                    },
                    RebaseStep {
                        oid: b,
                        action: RebaseAction::Drop,
                        message: None,
                    },
                ],
            },
        );
        let subjects = git.text(dir.path(), &["log", "--format=%s"]).unwrap();
        assert_eq!(subjects, "A\nbase");
        assert!(!dir.path().join("b").exists());
        assert!(dir.path().join("a").exists());
        assert!(dir.path().join("c").exists());
    }
}
