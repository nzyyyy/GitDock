use crate::{
    git::{self, Git},
    models::*,
    repositories::register_repository,
    AppState, RepositoryChanged,
};
use std::{
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, State};

pub(crate) struct RunningOperation {
    pub(crate) pid: u32,
    pub(crate) cancelled: Arc<AtomicBool>,
}

pub(crate) enum OperationContext {
    Repository {
        repository_id: RepositoryId,
        common_git_dir: PathBuf,
    },
    Clone {
        destination: PathBuf,
    },
}

pub(crate) struct CommandSpec {
    pub(crate) args: Vec<String>,
    pub(crate) input: Option<Vec<u8>>,
    pub(crate) env: Vec<(String, String)>,
    pub(crate) cleanup_dir: Option<PathBuf>,
}

pub(crate) fn acquire_lock(state: &AppState, common: &Path) -> Result<(), String> {
    let mut locks = state
        .write_locks
        .lock()
        .map_err(|_| "Repository lock is busy")?;
    if !locks.insert(common.to_path_buf()) {
        Err("Another write operation is already running in this repository".into())
    } else {
        Ok(())
    }
}
pub(crate) fn release_lock(state: &AppState, common: &Path) {
    if let Ok(mut locks) = state.write_locks.lock() {
        locks.remove(common);
    }
}

pub(crate) fn suppress_watch(state: &State<'_, AppState>, repository_id: RepositoryId) {
    if let Ok(mut repositories) = state.mutating_repositories.lock() {
        repositories.insert(repository_id);
    }
}

pub(crate) fn resume_watch(state: &State<'_, AppState>, repository_id: RepositoryId) {
    if let Ok(mut repositories) = state.mutating_repositories.lock() {
        repositories.remove(&repository_id);
    }
}

pub(crate) fn spawn_git_operation(
    git: &Git,
    cwd: &Path,
    spec: CommandSpec,
    operation_id: OperationId,
    title: &str,
    context: OperationContext,
    state: &State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let CommandSpec {
        args,
        input,
        env,
        cleanup_dir,
    } = spec;
    let repository_id = match &context {
        OperationContext::Repository { repository_id, .. } => Some(*repository_id),
        OperationContext::Clone { .. } => None,
    };
    let mut command = Command::new(&git.path);
    command
        .current_dir(cwd)
        .args(&args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in &env {
        command.env(key, value);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Cannot start Git: {e}"))?;
    if let Some(input) = input {
        if let Err(error) = child
            .stdin
            .take()
            .ok_or("Cannot open Git stdin")?
            .write_all(&input)
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.to_string());
        }
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut running = match state.running.lock() {
        Ok(running) => running,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Operation registry is busy".into());
        }
    };
    running.insert(
        operation_id,
        RunningOperation {
            pid: child.id(),
            cancelled: cancelled.clone(),
        },
    );
    drop(running);
    emit(
        &app,
        operation_id,
        repository_id,
        OperationEventKind::Started,
        title,
        None,
        None,
    );

    let stdout_app = app.clone();
    let stderr_app = app.clone();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_thread = thread::spawn(move || {
        stream(
            stdout,
            &stdout_app,
            operation_id,
            repository_id,
            OperationEventKind::Stdout,
        )
    });
    let stderr_thread = thread::spawn(move || {
        stream(
            stderr,
            &stderr_app,
            operation_id,
            repository_id,
            OperationEventKind::Stderr,
        )
    });
    thread::spawn(move || {
        let status = child.wait();
        let _ = stdout_thread.join();
        let _ = stderr_thread.join();
        let was_cancelled = cancelled.load(Ordering::Relaxed);
        let success = status.as_ref().is_ok_and(|status| status.success()) && !was_cancelled;
        let mut final_repository_id = repository_id;
        let (message, exit_code, outcome) = if was_cancelled {
            let message = match &context {
                OperationContext::Clone { destination } => format!(
                    "Clone cancelled. Partial destination was kept at {}",
                    destination.display()
                ),
                OperationContext::Repository { repository_id, .. } => {
                    cancelled_repository_message(&app, *repository_id)
                }
            };
            (
                message,
                status.ok().and_then(|status| status.code()),
                OperationOutcome::Cancelled,
            )
        } else if success {
            match &context {
                OperationContext::Clone { destination } => {
                    match register_repository(
                        destination.to_string_lossy().into_owned(),
                        &app.state::<AppState>(),
                        &app,
                    ) {
                        Ok(repository) => {
                            final_repository_id = Some(repository.id);
                            (
                                "Clone completed".into(),
                                Some(0),
                                OperationOutcome::Succeeded,
                            )
                        }
                        Err(error) => (
                            format!(
                                "Clone completed at {}, but registration failed: {error}",
                                destination.display()
                            ),
                            Some(1),
                            OperationOutcome::Failed,
                        ),
                    }
                }
                OperationContext::Repository { .. } => (
                    "Operation completed".into(),
                    Some(0),
                    OperationOutcome::Succeeded,
                ),
            }
        } else {
            let message = match (&context, status.as_ref()) {
                (OperationContext::Clone { destination }, _) => format!(
                    "Clone failed. Partial destination was kept at {}",
                    destination.display()
                ),
                (_, Err(error)) => error.to_string(),
                _ => "Git operation failed".into(),
            };
            (
                message,
                status.ok().and_then(|status| status.code()).or(Some(1)),
                OperationOutcome::Failed,
            )
        };
        emit(
            &app,
            operation_id,
            final_repository_id,
            OperationEventKind::Finished,
            &message,
            exit_code,
            Some(outcome),
        );
        match context {
            OperationContext::Repository {
                repository_id,
                common_git_dir,
            } => finish_operation(&app, operation_id, repository_id, &common_git_dir),
            OperationContext::Clone { .. } => {
                if let Ok(mut running) = app.state::<AppState>().running.lock() {
                    running.remove(&operation_id);
                }
            }
        }
        if let Some(dir) = cleanup_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    });
    Ok(())
}

pub(crate) fn finish_operation(
    app: &AppHandle,
    operation_id: OperationId,
    repository_id: RepositoryId,
    common: &Path,
) {
    let state = app.state::<AppState>();
    if let Ok(mut running) = state.running.lock() {
        running.remove(&operation_id);
    }
    resume_watch(&state, repository_id);
    release_lock(&state, common);
    if let Ok(mut snapshots) = state.snapshots.lock() {
        snapshots.retain(|_, s| s.repository_id != repository_id);
    }
    let _ = app.emit("repository-changed", RepositoryChanged { repository_id });
}

fn stream<R: std::io::Read>(
    reader: Option<R>,
    app: &AppHandle,
    operation_id: OperationId,
    repository_id: Option<RepositoryId>,
    kind: OperationEventKind,
) {
    let Some(reader) = reader else { return };
    read_stream_frames(reader, |line| {
        emit(
            app,
            operation_id,
            repository_id,
            kind.clone(),
            &git::redact_url(&line),
            None,
            None,
        );
    });
}

pub(crate) fn read_stream_frames<R: Read>(reader: R, mut on_frame: impl FnMut(&str)) {
    let mut frame = Vec::new();
    for byte in BufReader::new(reader).bytes().map_while(Result::ok) {
        if byte == b'\r' || byte == b'\n' {
            if !frame.is_empty() {
                on_frame(&String::from_utf8_lossy(&frame));
                frame.clear();
            }
        } else {
            frame.push(byte);
        }
    }
    if !frame.is_empty() {
        on_frame(&String::from_utf8_lossy(&frame));
    }
}

pub(crate) fn emit(
    app: &AppHandle,
    operation_id: OperationId,
    repository_id: Option<RepositoryId>,
    kind: OperationEventKind,
    message: &str,
    exit_code: Option<i32>,
    outcome: Option<OperationOutcome>,
) {
    let _ = app.emit(
        "operation-event",
        OperationEvent {
            operation_id,
            repository_id,
            kind,
            message: message.into(),
            exit_code,
            outcome,
        },
    );
}

#[cfg(unix)]
fn signal_process_group(pid: u32, signal: i32) -> Result<(), String> {
    let result = unsafe { libc::kill(-(pid as i32), signal) };
    if result == 0 {
        Ok(())
    } else {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error.to_string())
        }
    }
}

#[cfg(unix)]
fn wait_for_process_group(pid: u32, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if unsafe { libc::kill(-(pid as i32), 0) } != 0 {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
    true
}

fn cancelled_repository_message(app: &AppHandle, repository_id: RepositoryId) -> String {
    let state = app.state::<AppState>();
    let ongoing = state
        .record(repository_id)
        .ok()
        .and_then(|record| state.git().ok().map(|git| git.summary(&record)))
        .and_then(|summary| summary.ongoing);
    match ongoing.map(|state| state.kind) {
        Some(OngoingKind::Merge) => "Operation cancelled. Repository remains in a merge state.",
        Some(OngoingKind::Rebase) => "Operation cancelled. Repository remains in a rebase state.",
        Some(OngoingKind::CherryPick) => {
            "Operation cancelled. Repository remains in a cherry-pick state."
        }
        Some(OngoingKind::Revert) => "Operation cancelled. Repository remains in a revert state.",
        None => "Operation cancelled.",
    }
    .into()
}

#[cfg(unix)]
pub(crate) fn terminate_process_group(pid: u32) -> Result<(), String> {
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGKILL] {
        signal_process_group(pid, signal)?;
        if !wait_for_process_group(pid, Duration::from_millis(500)) {
            return Ok(());
        }
    }
    Err("Git process group did not exit after cancellation".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{git::Git, test_util::test_state};

    #[test]
    fn stream_frames_split_carriage_return_progress() {
        let mut frames = Vec::new();
        read_stream_frames(
            std::io::Cursor::new(b"Counting 1\rCounting 2\r\nDone\nTail"),
            |frame| frames.push(frame.to_string()),
        );
        assert_eq!(frames, ["Counting 1", "Counting 2", "Done", "Tail"]);
    }

    #[test]
    fn shared_git_directory_lock_rejects_a_second_writer() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(git, dir.path().join("config.json"));
        let common = dir.path().join("common.git");
        acquire_lock(&state, &common).unwrap();
        assert!(acquire_lock(&state, &common).is_err());
        release_lock(&state, &common);
        acquire_lock(&state, &common).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_terminates_the_full_process_group() {
        use std::os::unix::process::CommandExt;

        let mut child = Command::new("sh")
            .args(["-c", "trap '' INT TERM; while :; do sleep 1; done"])
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id();
        let waiter = thread::spawn(move || child.wait().unwrap());
        thread::sleep(Duration::from_millis(100));
        terminate_process_group(pid).unwrap();
        assert!(!waiter.join().unwrap().success());
    }
}
