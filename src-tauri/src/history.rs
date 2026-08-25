use crate::{
    git::{self, ensure_success},
    models::*,
    operations::verify_commit,
    repository_path::validate_relative_path,
    AppState,
};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
#[specta::specta]
pub(crate) fn get_history(
    repository_id: RepositoryId,
    cursor: Option<HistoryCursor>,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<CommitPage, String> {
    validate_history_cursor(&cursor)?;
    let git = state.git()?;
    let record = state.record(repository_id)?;
    git.history(Path::new(&record.path), cursor, limit.clamp(1, 200))
}

fn validate_history_cursor(cursor: &Option<HistoryCursor>) -> Result<(), String> {
    if cursor.as_ref().is_some_and(|cursor| {
        cursor.offset > 10_000_000
            || cursor.active_lanes.len() > 512
            || cursor.active_lanes.iter().any(|oid| {
                !oid.is_empty()
                    && (!matches!(oid.len(), 40 | 64)
                        || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()))
            })
    }) {
        Err("History cursor is invalid".into())
    } else {
        Ok(())
    }
}

const MAX_EXPORTED_LOG_LINES: usize = 10_000;
const MAX_EXPORTED_LOG_BYTES: usize = 5 * 1024 * 1024;

#[tauri::command]
#[specta::specta]
pub(crate) async fn export_session_log(
    file_name: String,
    lines: Vec<SessionLogLine>,
    app: AppHandle,
) -> Result<bool, String> {
    validate_log_file_name(&file_name)?;
    let dialog = app.clone();
    let path = tauri::async_runtime::spawn_blocking(move || {
        dialog
            .dialog()
            .file()
            .add_filter("Log", &["log"])
            .set_file_name(file_name)
            .blocking_save_file()
    })
    .await
    .map_err(|error| error.to_string())?;
    let Some(path) = path else { return Ok(false) };
    let path = path.into_path().map_err(|error| error.to_string())?;
    write_session_log(&path, lines)?;
    Ok(true)
}

fn validate_log_file_name(file_name: &str) -> Result<(), String> {
    if file_name.len() > 160
        || !file_name.ends_with(".log")
        || file_name.chars().any(char::is_control)
        || Path::new(file_name)
            .file_name()
            .and_then(|name| name.to_str())
            != Some(file_name)
    {
        Err("Session log file name is invalid".into())
    } else {
        Ok(())
    }
}

fn write_session_log(path: &Path, lines: Vec<SessionLogLine>) -> Result<(), String> {
    if !path.is_absolute() || !path.parent().is_some_and(Path::is_dir) || path.is_dir() {
        return Err("Choose a valid absolute log file path".into());
    }
    if lines.len() > MAX_EXPORTED_LOG_LINES {
        return Err("Session log is too large to export".into());
    }
    let mut output = String::new();
    for line in lines {
        if line.timestamp.len() > 64
            || line.timestamp.chars().any(char::is_whitespace)
            || !matches!(
                line.kind.as_str(),
                "started" | "stdout" | "stderr" | "finished" | "error"
            )
        {
            return Err("Session log contains an invalid entry".into());
        }
        let message = git::redact_url(&line.message.replace(['\r', '\n'], " "));
        output.push_str(&format!("{} {} {}\n", line.timestamp, line.kind, message));
        if output.len() > MAX_EXPORTED_LOG_BYTES {
            return Err("Session log is too large to export".into());
        }
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("log");
    let (temporary, mut file) = (0..100)
        .find_map(|attempt| {
            let temporary = path.with_extension(format!(
                "{extension}.{}.{}.tmp",
                std::process::id(),
                attempt
            ));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => Some(Ok((temporary, file))),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(error.to_string())),
            }
        })
        .ok_or("Cannot create a temporary log file")??;
    let result = (|| {
        file.write_all(output.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|error| error.to_string())?;
        fs::rename(&temporary, &path).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[tauri::command]
#[specta::specta]
pub(crate) fn get_commit_detail(
    repository_id: RepositoryId,
    oid: String,
    state: State<'_, AppState>,
) -> Result<CommitDetail, String> {
    let git = state.git()?;
    let record = state.record(repository_id)?;
    let cwd = Path::new(&record.path);
    verify_commit(&git, cwd, &oid)?;
    git.commit_detail(cwd, &oid)
}

#[tauri::command]
#[specta::specta]
pub(crate) fn get_rebase_commits(
    repository_id: RepositoryId,
    onto: String,
    state: State<'_, AppState>,
) -> Result<Vec<RebaseCommit>, String> {
    let git = state.git()?;
    let record = state.record(repository_id)?;
    let cwd = Path::new(&record.path);
    verify_commit(&git, cwd, &onto)?;
    git.rebase_commits(cwd, &onto)
}

#[tauri::command]
#[specta::specta]
pub(crate) fn get_file_history(
    repository_id: RepositoryId,
    path: String,
    state: State<'_, AppState>,
) -> Result<Vec<FileHistoryEntry>, String> {
    validate_relative_path(&path)?;
    let git = state.git()?;
    let record = state.record(repository_id)?;
    git.file_history(Path::new(&record.path), &path)
}

#[tauri::command]
#[specta::specta]
pub(crate) fn get_commit_file_diff(
    repository_id: RepositoryId,
    oid: String,
    path: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    validate_relative_path(&path)?;
    let git = state.git()?;
    let record = state.record(repository_id)?;
    let cwd = Path::new(&record.path);
    verify_commit(&git, cwd, &oid)?;
    git.commit_file_diff(cwd, &oid, &path)
}

#[tauri::command]
#[specta::specta]
pub(crate) fn get_blame(
    repository_id: RepositoryId,
    path: String,
    state: State<'_, AppState>,
) -> Result<BlameFile, String> {
    validate_relative_path(&path)?;
    let git = state.git()?;
    let record = state.record(repository_id)?;
    git.blame(Path::new(&record.path), &path)
}

#[tauri::command]
#[specta::specta]
pub(crate) fn compare_branches(
    repository_id: RepositoryId,
    base: String,
    head: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let git = state.git()?;
    let record = state.record(repository_id)?;
    let cwd = Path::new(&record.path);
    verify_commit(&git, cwd, &base)?;
    verify_commit(&git, cwd, &head)?;
    let merge_base = git.text(cwd, &["merge-base", &base, &head])?;
    let output = ensure_success(git.run(
        cwd,
        &[
            "diff".into(),
            "--no-ext-diff".into(),
            "--no-color".into(),
            format!("{base}..{head}"),
            "--".into(),
        ],
        None,
    )?)?;
    let patch = String::from_utf8_lossy(&output.stdout);
    if patch.len() > git::MAX_DIFF_BYTES || patch.lines().count() > git::MAX_DIFF_LINES {
        return Err("This comparison is too large. Open it with the configured difftool.".into());
    }
    Ok(format!("merge base {merge_base}\n\n{patch}"))
}

#[tauri::command]
#[specta::specta]
pub(crate) fn open_repository_file(
    repository_id: RepositoryId,
    path: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    validate_relative_path(&path)?;
    let record = state.record(repository_id)?;
    let root = PathBuf::from(record.path)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let target = root.join(path);
    if !target.exists() {
        return Err("The selected file no longer exists".into());
    }
    app.opener()
        .open_path(target.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) fn get_branches(
    repository_id: RepositoryId,
    state: State<'_, AppState>,
) -> Result<Vec<BranchInfo>, String> {
    let g = state.git()?;
    g.branches(Path::new(&state.record(repository_id)?.path))
}
#[tauri::command]
#[specta::specta]
pub(crate) fn get_tags(
    repository_id: RepositoryId,
    state: State<'_, AppState>,
) -> Result<Vec<TagInfo>, String> {
    let g = state.git()?;
    g.tags(Path::new(&state.record(repository_id)?.path))
}
#[tauri::command]
#[specta::specta]
pub(crate) fn get_remotes(
    repository_id: RepositoryId,
    state: State<'_, AppState>,
) -> Result<Vec<RemoteInfo>, String> {
    let g = state.git()?;
    g.remotes(Path::new(&state.record(repository_id)?.path))
}
#[tauri::command]
#[specta::specta]
pub(crate) fn get_stashes(
    repository_id: RepositoryId,
    state: State<'_, AppState>,
) -> Result<Vec<StashInfo>, String> {
    let g = state.git()?;
    g.stashes(Path::new(&state.record(repository_id)?.path))
}
#[tauri::command]
#[specta::specta]
pub(crate) fn get_submodules(
    repository_id: RepositoryId,
    state: State<'_, AppState>,
) -> Result<Vec<SubmoduleInfo>, String> {
    let g = state.git()?;
    g.submodules(Path::new(&state.record(repository_id)?.path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn rejects_untrusted_history_lane_state() {
        assert!(validate_history_cursor(&Some(HistoryCursor {
            offset: 100,
            active_lanes: vec!["not-an-oid".into()],
        }))
        .is_err());
        assert!(validate_history_cursor(&Some(HistoryCursor {
            offset: 100,
            active_lanes: vec!["a".repeat(40)],
        }))
        .is_ok());
        assert!(validate_history_cursor(&Some(HistoryCursor {
            offset: 100,
            active_lanes: vec!["a".repeat(40), String::new(), "b".repeat(40)],
        }))
        .is_ok());
    }

    #[test]
    fn exports_a_bounded_redacted_session_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.log");
        write_session_log(
            &path,
            vec![SessionLogLine {
                timestamp: "2026-08-09T08:00:00.000Z".into(),
                kind: "stderr".into(),
                message: "https://user:token@example.com/repo".into(),
            }],
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "2026-08-09T08:00:00.000Z stderr https://***@example.com/repo\n"
        );
        assert!(write_session_log(
            &dir.path().join("invalid.log"),
            vec![SessionLogLine {
                timestamp: "2026-08-09T08:00:00.000Z".into(),
                kind: "untrusted".into(),
                message: "message".into(),
            }],
        )
        .is_err());
        assert!(write_session_log(Path::new("relative.log"), Vec::new()).is_err());
        assert!(validate_log_file_name("../session.log").is_err());
        assert!(validate_log_file_name("session.log").is_ok());
    }
}
