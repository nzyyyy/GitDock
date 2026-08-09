mod git;
mod models;
mod store;

use crate::{
    git::{ensure_success, path_name, strings, Git},
    models::*,
    store::ConfigStore,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufReader, Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

pub struct AppState {
    store: Mutex<ConfigStore>,
    git: Mutex<Result<Git, String>>,
    snapshots: Mutex<HashMap<u64, SnapshotCache>>,
    next_snapshot_id: AtomicU64,
    next_operation_id: AtomicU64,
    write_locks: Mutex<HashSet<PathBuf>>,
    mutating_repositories: Mutex<HashSet<RepositoryId>>,
    running: Mutex<HashMap<OperationId, RunningOperation>>,
    watcher: Mutex<Option<RecommendedWatcher>>,
}

#[derive(Clone)]
struct SnapshotCache {
    repository_id: RepositoryId,
    head_oid: Option<String>,
    hunks: HashMap<String, CachedHunk>,
}

#[derive(Clone)]
struct CachedHunk {
    path: String,
    staged: bool,
    patch: Vec<u8>,
    source_diff: String,
}

struct RunningOperation {
    pid: u32,
    cancelled: Arc<AtomicBool>,
}

enum OperationContext {
    Repository {
        repository_id: RepositoryId,
        common_git_dir: PathBuf,
    },
    Clone {
        destination: PathBuf,
    },
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RepositoryChanged {
    repository_id: RepositoryId,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RepositoryListChanged;

struct CommandSpec {
    args: Vec<String>,
    input: Option<Vec<u8>>,
}

impl AppState {
    fn git(&self) -> Result<Git, String> {
        self.git
            .lock()
            .map_err(|_| "Git state is unavailable".to_string())?
            .clone()
    }

    fn record(&self, id: RepositoryId) -> Result<RepositoryRecord, String> {
        self.store
            .lock()
            .map_err(|_| "Settings are busy".to_string())?
            .config
            .repositories
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| "Repository is not registered".into())
    }
}

#[tauri::command]
fn bootstrap(state: State<'_, AppState>) -> Result<Bootstrap, String> {
    let store = state.store.lock().map_err(|_| "Settings are busy")?;
    let git = state.git.lock().map_err(|_| "Git state is busy")?;
    let repositories = git
        .as_ref()
        .map(|git| {
            store
                .config
                .repositories
                .iter()
                .map(|r| git.summary(r))
                .collect()
        })
        .unwrap_or_default();
    Ok(Bootstrap {
        git: Git::info(&git),
        settings: store.config.settings.clone(),
        repositories,
    })
}

#[tauri::command]
fn refresh_repositories(
    active_repository_id: Option<RepositoryId>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<Vec<RepositorySummary>, String> {
    let git = state.git()?;
    let mut records = state
        .store
        .lock()
        .map_err(|_| "Settings are busy")?
        .config
        .repositories
        .clone();
    prioritize_repository(&mut records, active_repository_id);
    let mut summaries = Vec::with_capacity(records.len());
    for (index, chunk) in records.chunks(4).enumerate() {
        let batch = thread::scope(|scope| {
            chunk
                .iter()
                .map(|record| {
                    let git = git.clone();
                    scope.spawn(move || git.summary(record))
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| handle.join().expect("repository refresh worker panicked"))
                .collect::<Vec<_>>()
        });
        if index == 0 {
            if let Some(summary) =
                active_repository_id.and_then(|id| batch.iter().find(|summary| summary.id == id))
            {
                let _ = app.emit("repository-summary-refreshed", summary.clone());
            }
        }
        summaries.extend(batch);
    }
    Ok(summaries)
}

fn prioritize_repository(
    records: &mut [RepositoryRecord],
    active_repository_id: Option<RepositoryId>,
) {
    if let Some(index) =
        active_repository_id.and_then(|id| records.iter().position(|record| record.id == id))
    {
        records.swap(0, index);
    }
}

#[tauri::command]
fn refresh_repository(
    repository_id: RepositoryId,
    state: State<'_, AppState>,
) -> Result<RepositorySummary, String> {
    Ok(state.git()?.summary(&state.record(repository_id)?))
}

#[tauri::command]
fn set_git_path(path: Option<String>, state: State<'_, AppState>) -> Result<GitInfo, String> {
    let discovered = Git::discover(path.as_deref());
    if discovered.is_err() {
        return Ok(Git::info(&discovered));
    }
    {
        let mut store = state.store.lock().map_err(|_| "Settings are busy")?;
        store.config.settings.git_path = path;
        store.save()?;
    }
    *state.git.lock().map_err(|_| "Git state is busy")? = discovered.clone();
    Ok(Git::info(&discovered))
}

#[tauri::command]
fn save_layout(
    left_width: u16,
    right_width: u16,
    output_height: u16,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|_| "Settings are busy")?;
    store.config.settings.left_width = left_width.clamp(190, 420);
    store.config.settings.right_width = right_width.clamp(300, 560);
    store.config.settings.output_height = output_height.clamp(120, 420);
    store.save()
}

#[tauri::command]
fn save_language(language: Language, state: State<'_, AppState>) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|_| "Settings are busy")?;
    store.config.settings.language = language;
    store.save()
}

#[tauri::command]
fn add_repository(
    path: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<RepositorySummary, String> {
    register_repository(path, &state, &app)
}

fn register_repository(
    path: String,
    state: &AppState,
    app: &AppHandle,
) -> Result<RepositorySummary, String> {
    let git = state.git()?;
    let inspection = git.inspect_repository(Path::new(&path))?;
    let canonical = inspection.root.to_string_lossy().to_string();
    let record = {
        let mut store = state.store.lock().map_err(|_| "Settings are busy")?;
        if store
            .config
            .repositories
            .iter()
            .any(|r| r.path == canonical)
        {
            return Err("This repository is already registered".into());
        }
        let id = store.config.next_repository_id;
        store.config.next_repository_id += 1;
        let record = RepositoryRecord {
            id,
            path: canonical,
            name: path_name(&inspection.root),
            group: None,
            favorite: false,
            order: store.config.repositories.len() as u32,
        };
        store.config.repositories.push(record.clone());
        store.config.settings.selected_repository_id = Some(id);
        store.save()?;
        record
    };
    let _ = app.emit("repository-list-changed", RepositoryListChanged);
    Ok(git.summary(&record))
}

#[tauri::command]
fn initialize_repository(
    path: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<RepositorySummary, String> {
    let git = state.git()?;
    std::fs::create_dir_all(&path)
        .map_err(|e| format!("Cannot create repository directory: {e}"))?;
    ensure_success(git.run(Path::new(&path), &strings(&["init"]), None)?)?;
    add_repository(path, state, app)
}

#[tauri::command]
fn clone_repository(
    url: String,
    destination: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<OperationResult, String> {
    if url.trim().is_empty() {
        return Err("Remote URL is required".into());
    }
    let git = state.git()?;
    let destination_path = PathBuf::from(&destination);
    let parent = destination_path
        .parent()
        .ok_or("Choose a destination directory")?
        .to_path_buf();
    std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let operation_id = state.next_operation_id.fetch_add(1, Ordering::Relaxed);
    spawn_git_operation(
        &git,
        &parent,
        CommandSpec {
            args: vec![
                "clone".into(),
                "--progress".into(),
                "--".into(),
                url,
                destination_path.to_string_lossy().into(),
            ],
            input: None,
        },
        operation_id,
        "Clone repository",
        OperationContext::Clone {
            destination: destination_path,
        },
        &state,
        app,
    )?;
    Ok(OperationResult {
        operation_id,
        accepted: true,
    })
}

#[tauri::command]
fn relocate_repository(
    repository_id: RepositoryId,
    path: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<RepositorySummary, String> {
    let git = state.git()?;
    let inspection = git.inspect_repository(Path::new(&path))?;
    let canonical = inspection.root.to_string_lossy().to_string();
    let record = {
        let mut store = state.store.lock().map_err(|_| "Settings are busy")?;
        if store
            .config
            .repositories
            .iter()
            .any(|r| r.id != repository_id && r.path == canonical)
        {
            return Err("This repository is already registered".into());
        }
        let record = store
            .config
            .repositories
            .iter_mut()
            .find(|r| r.id == repository_id)
            .ok_or("Repository is not registered")?;
        record.path = canonical;
        record.name = path_name(&inspection.root);
        let result = record.clone();
        store.save()?;
        result
    };
    let _ = app.emit("repository-list-changed", RepositoryListChanged);
    Ok(git.summary(&record))
}

#[tauri::command]
fn update_repository(
    repository: RepositoryRecord,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|_| "Settings are busy")?;
    let existing = store
        .config
        .repositories
        .iter_mut()
        .find(|r| r.id == repository.id)
        .ok_or("Repository is not registered")?;
    existing.name = repository.name;
    existing.group = repository.group;
    existing.favorite = repository.favorite;
    existing.order = repository.order;
    store.save()?;
    let _ = app.emit("repository-list-changed", RepositoryListChanged);
    Ok(())
}

#[tauri::command]
fn reorder_repositories(
    placements: Vec<RepositoryPlacement>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|_| "Settings are busy")?;
    persist_repository_placements(&mut store, &placements)?;
    let _ = app.emit("repository-list-changed", RepositoryListChanged);
    Ok(())
}

fn persist_repository_placements(
    store: &mut ConfigStore,
    placements: &[RepositoryPlacement],
) -> Result<(), String> {
    let previous = store.config.repositories.clone();
    apply_repository_placements(&mut store.config.repositories, placements)?;
    if let Err(error) = store.save() {
        store.config.repositories = previous;
        return Err(error);
    }
    Ok(())
}

fn apply_repository_placements(
    repositories: &mut [RepositoryRecord],
    placements: &[RepositoryPlacement],
) -> Result<(), String> {
    if placements.len() != repositories.len() {
        return Err("Repository order must include every registered repository".into());
    }
    let ids = placements
        .iter()
        .map(|placement| placement.id)
        .collect::<HashSet<_>>();
    let orders = placements
        .iter()
        .map(|placement| placement.order)
        .collect::<HashSet<_>>();
    if ids.len() != placements.len()
        || orders.len() != placements.len()
        || !placements
            .iter()
            .all(|placement| repositories.iter().any(|record| record.id == placement.id))
        || !(0..placements.len() as u32).all(|order| orders.contains(&order))
    {
        return Err("Repository order is incomplete or contains duplicates".into());
    }
    if placements.iter().any(|placement| {
        placement
            .group
            .as_deref()
            .is_some_and(|group| group.len() > 100 || group.chars().any(char::is_control))
    }) {
        return Err("Repository group must be at most 100 characters".into());
    }
    for placement in placements {
        let repository = repositories
            .iter_mut()
            .find(|repository| repository.id == placement.id)
            .expect("placement IDs were validated");
        repository.group = placement
            .group
            .as_deref()
            .map(str::trim)
            .filter(|group| !group.is_empty())
            .map(Into::into);
        repository.favorite = placement.favorite;
        repository.order = placement.order;
    }
    Ok(())
}

#[tauri::command]
fn remove_repository(
    repository_id: RepositoryId,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|_| "Settings are busy")?;
    store.config.repositories.retain(|r| r.id != repository_id);
    if store.config.settings.selected_repository_id == Some(repository_id) {
        store.config.settings.selected_repository_id = None;
    }
    store.save()?;
    let _ = app.emit("repository-list-changed", RepositoryListChanged);
    Ok(())
}

#[tauri::command]
fn watch_repository(
    repository_id: RepositoryId,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let record = state.record(repository_id)?;
    let path = PathBuf::from(record.path);
    let (events, pending) = mpsc::channel();
    let watch_app = app.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let suppressed = watch_app
            .state::<AppState>()
            .mutating_repositories
            .lock()
            .map(|repositories| repositories.contains(&repository_id))
            .unwrap_or(true);
        if event.is_ok() && !suppressed {
            let _ = events.send(());
        }
    })
    .map_err(|e| e.to_string())?;
    let event_app = app.clone();
    thread::spawn(move || {
        while wait_for_quiet(&pending) {
            let _ = event_app.emit("repository-changed", RepositoryChanged { repository_id });
        }
    });
    watcher
        .watch(&path, RecursiveMode::Recursive)
        .map_err(|e| e.to_string())?;
    *state.watcher.lock().map_err(|_| "File watcher is busy")? = Some(watcher);
    Ok(())
}

fn wait_for_quiet(events: &mpsc::Receiver<()>) -> bool {
    if events.recv().is_err() {
        return false;
    }
    while events.recv_timeout(Duration::from_millis(300)).is_ok() {}
    true
}

#[tauri::command]
fn get_status(
    repository_id: RepositoryId,
    include_ignored: bool,
    state: State<'_, AppState>,
) -> Result<WorkingTreeSnapshot, String> {
    let git = state.git()?;
    let record = state.record(repository_id)?;
    let inspection = git.inspect_repository(Path::new(&record.path))?;
    if inspection.bare {
        return Err("Bare repositories do not have a working tree".into());
    }
    let id = state.next_snapshot_id.fetch_add(1, Ordering::Relaxed);
    let snapshot = git.status(repository_id, &inspection.root, include_ignored, id)?;
    state
        .snapshots
        .lock()
        .map_err(|_| "Snapshot cache is busy")?
        .insert(
            id,
            SnapshotCache {
                repository_id,
                head_oid: snapshot.head_oid.clone(),
                hunks: HashMap::new(),
            },
        );
    Ok(snapshot)
}

#[tauri::command]
fn get_diff(
    repository_id: RepositoryId,
    snapshot_id: u64,
    path: String,
    staged: bool,
    state: State<'_, AppState>,
) -> Result<DiffFile, String> {
    validate_relative_path(&path)?;
    let git = state.git()?;
    let record = state.record(repository_id)?;
    let mut snapshots = state
        .snapshots
        .lock()
        .map_err(|_| "Snapshot cache is busy")?;
    let snapshot = snapshots
        .get_mut(&snapshot_id)
        .ok_or("This view is stale. Refresh the repository and try again.")?;
    if snapshot.repository_id != repository_id {
        return Err("Snapshot does not belong to this repository".into());
    }
    let current_head = git
        .text(Path::new(&record.path), &["rev-parse", "--verify", "HEAD"])
        .ok();
    if snapshot.head_oid != current_head {
        return Err("HEAD changed. Refresh the repository and try again.".into());
    }
    let diff = git.diff(Path::new(&record.path), &path, staged, snapshot_id)?;
    for hunk in &diff.hunks {
        snapshot.hunks.insert(
            hunk.id.clone(),
            CachedHunk {
                path: path.clone(),
                staged,
                patch: hunk.patch.as_bytes().to_vec(),
                source_diff: diff.patch.clone(),
            },
        );
    }
    Ok(diff)
}

#[tauri::command]
fn get_history(
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
                !matches!(oid.len(), 40 | 64) || !oid.bytes().all(|byte| byte.is_ascii_hexdigit())
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
async fn export_session_log(
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
fn get_commit_diff(
    repository_id: RepositoryId,
    oid: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let git = state.git()?;
    let record = state.record(repository_id)?;
    verify_commit(&git, Path::new(&record.path), &oid)?;
    let output = ensure_success(git.run(
        Path::new(&record.path),
        &[
            "show".into(),
            "--no-ext-diff".into(),
            "--no-color".into(),
            "--format=fuller".into(),
            "--stat".into(),
            "--patch".into(),
            oid,
            "--".into(),
        ],
        None,
    )?)?;
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    if text.len() > git::MAX_DIFF_BYTES || text.lines().count() > git::MAX_DIFF_LINES {
        Err("This commit diff is too large. Open it with the configured difftool.".into())
    } else {
        Ok(text)
    }
}

#[tauri::command]
fn compare_branches(
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
            format!("{base}...{head}"),
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
fn open_repository_file(
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
fn get_branches(
    repository_id: RepositoryId,
    state: State<'_, AppState>,
) -> Result<Vec<BranchInfo>, String> {
    let g = state.git()?;
    g.branches(Path::new(&state.record(repository_id)?.path))
}
#[tauri::command]
fn get_tags(
    repository_id: RepositoryId,
    state: State<'_, AppState>,
) -> Result<Vec<TagInfo>, String> {
    let g = state.git()?;
    g.tags(Path::new(&state.record(repository_id)?.path))
}
#[tauri::command]
fn get_remotes(
    repository_id: RepositoryId,
    state: State<'_, AppState>,
) -> Result<Vec<RemoteInfo>, String> {
    let g = state.git()?;
    g.remotes(Path::new(&state.record(repository_id)?.path))
}
#[tauri::command]
fn get_stashes(
    repository_id: RepositoryId,
    state: State<'_, AppState>,
) -> Result<Vec<StashInfo>, String> {
    let g = state.git()?;
    g.stashes(Path::new(&state.record(repository_id)?.path))
}
#[tauri::command]
fn get_submodules(
    repository_id: RepositoryId,
    state: State<'_, AppState>,
) -> Result<Vec<SubmoduleInfo>, String> {
    let g = state.git()?;
    g.submodules(Path::new(&state.record(repository_id)?.path))
}

#[tauri::command]
fn preview_operation(
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
fn start_operation(
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
            let result = paths
                .iter()
                .try_for_each(|path| trash::delete(root.join(path)).map_err(|e| e.to_string()));
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
    acquire_lock(&state, &inspection.common_git_dir)?;
    suppress_watch(&state, repository_id);
    spawn_git_operation(
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
    )
    .map_err(|error| {
        release_lock(&state, &inspection.common_git_dir);
        resume_watch(&state, repository_id);
        error
    })?;
    Ok(OperationResult {
        operation_id,
        accepted: true,
    })
}

#[tauri::command]
fn cancel_operation(operation_id: OperationId, state: State<'_, AppState>) -> Result<(), String> {
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

fn command_spec(
    state: &AppState,
    repository_id: RepositoryId,
    cwd: &Path,
    request: &OperationRequest,
) -> Result<CommandSpec, String> {
    let mut input = None;
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
        OperationRequest::TrashUntracked { .. } => unreachable!(),
    };
    Ok(CommandSpec { args, input })
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
        _ => Ok(()),
    }
}

fn validate_relative_path(path: &str) -> Result<(), String> {
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
fn verify_commit(git: &Git, cwd: &Path, oid: &str) -> Result<(), String> {
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

fn acquire_lock(state: &AppState, common: &Path) -> Result<(), String> {
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
fn release_lock(state: &AppState, common: &Path) {
    if let Ok(mut locks) = state.write_locks.lock() {
        locks.remove(common);
    }
}

fn suppress_watch(state: &State<'_, AppState>, repository_id: RepositoryId) {
    if let Ok(mut repositories) = state.mutating_repositories.lock() {
        repositories.insert(repository_id);
    }
}

fn resume_watch(state: &State<'_, AppState>, repository_id: RepositoryId) {
    if let Ok(mut repositories) = state.mutating_repositories.lock() {
        repositories.remove(&repository_id);
    }
}

fn spawn_git_operation(
    git: &Git,
    cwd: &Path,
    spec: CommandSpec,
    operation_id: OperationId,
    title: &str,
    context: OperationContext,
    state: &State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let repository_id = match &context {
        OperationContext::Repository { repository_id, .. } => Some(*repository_id),
        OperationContext::Clone { .. } => None,
    };
    let mut command = Command::new(&git.path);
    command
        .current_dir(cwd)
        .args(&spec.args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .stdin(if spec.input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Cannot start Git: {e}"))?;
    if let Some(input) = spec.input {
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
    });
    Ok(())
}

fn finish_operation(
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

fn read_stream_frames<R: Read>(reader: R, mut on_frame: impl FnMut(&str)) {
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

fn emit(
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
fn terminate_process_group(pid: u32) -> Result<(), String> {
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGKILL] {
        signal_process_group(pid, signal)?;
        if !wait_for_process_group(pid, Duration::from_millis(500)) {
            return Ok(());
        }
    }
    Err("Git process group did not exit after cancellation".into())
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
        | OperationRequest::RunDifftool { path, .. } => vec![path.clone()],
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
        _ => "Run Git operation",
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let config_path = app.path().app_config_dir()?.join("config.json");
            let store = ConfigStore::load(config_path).map_err(std::io::Error::other)?;
            let git = Git::discover(store.config.settings.git_path.as_deref());
            app.manage(AppState {
                store: Mutex::new(store),
                git: Mutex::new(git),
                snapshots: Mutex::new(HashMap::new()),
                next_snapshot_id: AtomicU64::new(1),
                next_operation_id: AtomicU64::new(1),
                write_locks: Mutex::new(HashSet::new()),
                mutating_repositories: Mutex::new(HashSet::new()),
                running: Mutex::new(HashMap::new()),
                watcher: Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            refresh_repositories,
            refresh_repository,
            set_git_path,
            save_layout,
            save_language,
            add_repository,
            initialize_repository,
            clone_repository,
            relocate_repository,
            update_repository,
            reorder_repositories,
            remove_repository,
            watch_repository,
            get_status,
            get_diff,
            get_history,
            export_session_log,
            get_commit_diff,
            compare_branches,
            open_repository_file,
            get_branches,
            get_tags,
            get_remotes,
            get_stashes,
            get_submodules,
            preview_operation,
            start_operation,
            cancel_operation
        ])
        .run(tauri::generate_context!())
        .expect("error while running GitDock");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state(git: Git, config_path: PathBuf) -> AppState {
        AppState {
            store: Mutex::new(ConfigStore::load(config_path).unwrap()),
            git: Mutex::new(Ok(git)),
            snapshots: Mutex::new(HashMap::new()),
            next_snapshot_id: AtomicU64::new(1),
            next_operation_id: AtomicU64::new(1),
            write_locks: Mutex::new(HashSet::new()),
            mutating_repositories: Mutex::new(HashSet::new()),
            running: Mutex::new(HashMap::new()),
            watcher: Mutex::new(None),
        }
    }

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
        let preview = preview(
            &record,
            &OperationRequest::DiscardTracked {
                paths: vec!["a".into()],
            },
        )
        .unwrap();
        assert_eq!(preview.risk, RiskLevel::Destructive);
        assert!(preview.requires_confirmation);
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
        assert_eq!(requests.len(), 37);
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
    fn watcher_waits_until_the_event_burst_is_quiet() {
        let (sender, receiver) = mpsc::channel();
        let burst = sender.clone();
        let started = std::time::Instant::now();
        thread::spawn(move || {
            burst.send(()).unwrap();
            thread::sleep(Duration::from_millis(100));
            burst.send(()).unwrap();
            thread::sleep(Duration::from_millis(100));
            burst.send(()).unwrap();
        });
        assert!(wait_for_quiet(&receiver));
        assert!(started.elapsed() >= Duration::from_millis(450));
        drop(sender);
    }

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
    fn prioritizes_the_active_repository_in_a_fifty_repository_refresh() {
        let mut records = (0..50)
            .map(|id| RepositoryRecord {
                id,
                path: format!("/tmp/{id}"),
                name: id.to_string(),
                group: None,
                favorite: false,
                order: id as u32,
            })
            .collect::<Vec<_>>();
        prioritize_repository(&mut records, Some(37));
        assert_eq!(records[0].id, 37);
        assert_eq!(records.len(), 50);
    }

    #[test]
    fn repository_placements_are_complete_and_atomic() {
        let original = vec![
            RepositoryRecord {
                id: 1,
                path: "a".into(),
                name: "a".into(),
                group: None,
                favorite: false,
                order: 0,
            },
            RepositoryRecord {
                id: 2,
                path: "b".into(),
                name: "b".into(),
                group: Some("Work".into()),
                favorite: false,
                order: 1,
            },
        ];
        let mut repositories = original.clone();
        assert!(apply_repository_placements(
            &mut repositories,
            &[RepositoryPlacement {
                id: 1,
                group: None,
                favorite: true,
                order: 0
            }],
        )
        .is_err());
        assert_eq!(repositories, original);

        apply_repository_placements(
            &mut repositories,
            &[
                RepositoryPlacement {
                    id: 2,
                    group: Some("Team".into()),
                    favorite: false,
                    order: 0,
                },
                RepositoryPlacement {
                    id: 1,
                    group: None,
                    favorite: true,
                    order: 1,
                },
            ],
        )
        .unwrap();
        assert_eq!(repositories[0].order, 1);
        assert!(repositories[0].favorite);
        assert_eq!(repositories[1].group.as_deref(), Some("Team"));
    }

    #[test]
    fn repository_placements_roll_back_when_saving_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut store = ConfigStore::load(path.clone()).unwrap();
        store.config.repositories = vec![RepositoryRecord {
            id: 1,
            path: "a".into(),
            name: "a".into(),
            group: None,
            favorite: false,
            order: 0,
        }];
        store.save().unwrap();
        fs::write(path, "damaged").unwrap();
        let previous = store.config.repositories.clone();
        assert!(persist_repository_placements(
            &mut store,
            &[RepositoryPlacement {
                id: 1,
                group: Some("Team".into()),
                favorite: true,
                order: 0,
            }],
        )
        .is_err());
        assert_eq!(store.config.repositories, previous);
    }

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
