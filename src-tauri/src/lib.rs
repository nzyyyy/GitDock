mod git;
mod models;
mod store;

use crate::{
    git::{
        ensure_success, file_executable, path_name, render_conflict_resolution, strings,
        ConflictSource, Git,
    },
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
        mpsc, Arc, Condvar, Mutex,
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
    summary_refresh: Mutex<SummaryRefreshState>,
    summary_refresh_running: Mutex<usize>,
    summary_refresh_ready: Condvar,
}

#[derive(Default)]
struct SummaryRefreshState {
    generation: u64,
    cache: HashMap<RepositoryId, RepositorySummary>,
}

struct SummaryRefreshPermit<'a>(&'a AppState);

impl Drop for SummaryRefreshPermit<'_> {
    fn drop(&mut self) {
        if let Ok(mut running) = self.0.summary_refresh_running.lock() {
            *running -= 1;
            self.0.summary_refresh_ready.notify_one();
        }
    }
}

#[derive(Clone)]
struct SnapshotCache {
    repository_id: RepositoryId,
    head_oid: Option<String>,
    hunks: HashMap<String, CachedHunk>,
    conflicts: HashMap<String, ConflictSource>,
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
    let (settings, records) = {
        let store = state.store.lock().map_err(|_| "Settings are busy")?;
        (
            store.config.settings.clone(),
            store.config.repositories.clone(),
        )
    };
    let git = state.git.lock().map_err(|_| "Git state is busy")?.clone();
    let repositories: Vec<RepositorySummary> = git
        .as_ref()
        .map(|git| records.iter().map(|record| git.summary(record)).collect())
        .unwrap_or_default();
    state
        .summary_refresh
        .lock()
        .map_err(|_| "Repository summary cache is busy")?
        .cache = repositories
        .iter()
        .map(|summary| (summary.id, summary.clone()))
        .collect();
    Ok(Bootstrap {
        git: Git::info(&git),
        settings,
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
    let records = state
        .store
        .lock()
        .map_err(|_| "Settings are busy")?
        .config
        .repositories
        .clone();
    let generation = start_summary_refresh(&state)?;
    let active = active_repository_id
        .and_then(|id| records.iter().find(|record| record.id == id))
        .map(|record| git.summary(record));
    let mut refresh = state
        .summary_refresh
        .lock()
        .map_err(|_| "Repository summary cache is busy")?;
    refresh
        .cache
        .retain(|id, _| records.iter().any(|record| record.id == *id));
    let current = refresh.generation == generation;
    if current {
        if let Some(summary) = &active {
            refresh.cache.insert(summary.id, summary.clone());
            let _ = app.emit("repository-summary-refreshed", summary.clone());
        }
    }
    let summaries = records
        .iter()
        .map(|record| {
            refresh
                .cache
                .get(&record.id)
                .filter(|summary| summary.path == record.path)
                .cloned()
                .map(|summary| summary_with_record(summary, record))
                .unwrap_or_else(|| {
                    let summary = git.summary(record);
                    if current {
                        refresh.cache.insert(record.id, summary.clone());
                    }
                    summary
                })
        })
        .collect::<Vec<_>>();
    drop(refresh);

    let inactive = records
        .into_iter()
        .filter(|record| Some(record.id) != active_repository_id)
        .collect::<Vec<_>>();
    if current && !inactive.is_empty() {
        thread::spawn(move || {
            for chunk in inactive.chunks(4) {
                let state = app.state::<AppState>();
                if !summary_refresh_is_current(&state, generation) {
                    return;
                }
                let batch = thread::scope(|scope| {
                    chunk
                        .iter()
                        .map(|record| {
                            let git = git.clone();
                            let state = &state;
                            scope.spawn(move || {
                                let _permit = acquire_summary_refresh_permit(state)?;
                                summary_refresh_is_current(state, generation)
                                    .then(|| git.summary(record))
                                    .ok_or_else(|| "stale summary refresh".to_string())
                            })
                        })
                        .collect::<Vec<_>>()
                        .into_iter()
                        .filter_map(|handle| {
                            handle
                                .join()
                                .expect("repository refresh worker panicked")
                                .ok()
                        })
                        .collect::<Vec<_>>()
                });
                if !publish_summary_batch(&state, generation, &batch, |summary| {
                    let _ = app.emit("repository-summary-refreshed", summary.clone());
                }) {
                    return;
                }
            }
        });
    }
    Ok(summaries)
}

fn summary_with_record(
    mut summary: RepositorySummary,
    record: &RepositoryRecord,
) -> RepositorySummary {
    summary.path = record.path.clone();
    summary.name = record.name.clone();
    summary.group = record.group.clone();
    summary.favorite = record.favorite;
    summary.order = record.order;
    summary
}

fn start_summary_refresh(state: &AppState) -> Result<u64, String> {
    let mut refresh = state
        .summary_refresh
        .lock()
        .map_err(|_| "Repository summary cache is busy")?;
    refresh.generation += 1;
    Ok(refresh.generation)
}

fn invalidate_summary_refresh(state: &AppState) {
    if let Ok(mut refresh) = state.summary_refresh.lock() {
        refresh.generation += 1;
    }
}

fn summary_refresh_is_current(state: &AppState, generation: u64) -> bool {
    state
        .summary_refresh
        .lock()
        .is_ok_and(|refresh| refresh.generation == generation)
}

fn publish_summary_batch(
    state: &AppState,
    generation: u64,
    summaries: &[RepositorySummary],
    mut publish: impl FnMut(&RepositorySummary),
) -> bool {
    let Ok(mut refresh) = state.summary_refresh.lock() else {
        return false;
    };
    if refresh.generation != generation {
        return false;
    }
    for summary in summaries {
        refresh.cache.insert(summary.id, summary.clone());
        publish(summary);
    }
    true
}

fn acquire_summary_refresh_permit(state: &AppState) -> Result<SummaryRefreshPermit<'_>, String> {
    let mut running = state
        .summary_refresh_running
        .lock()
        .map_err(|_| "Repository refresh limiter is busy")?;
    while *running >= 4 {
        running = state
            .summary_refresh_ready
            .wait(running)
            .map_err(|_| "Repository refresh limiter is busy")?;
    }
    *running += 1;
    Ok(SummaryRefreshPermit(state))
}

fn replace_cached_summary(
    state: &AppState,
    summary: RepositorySummary,
) -> Result<RepositorySummary, String> {
    let mut refresh = state
        .summary_refresh
        .lock()
        .map_err(|_| "Repository summary cache is busy")?;
    refresh.generation += 1;
    refresh.cache.insert(summary.id, summary.clone());
    Ok(summary)
}

fn remove_cached_summary(state: &AppState, repository_id: RepositoryId) {
    if let Ok(mut refresh) = state.summary_refresh.lock() {
        refresh.generation += 1;
        refresh.cache.remove(&repository_id);
    }
}

fn clear_summary_cache(state: &AppState) {
    if let Ok(mut refresh) = state.summary_refresh.lock() {
        refresh.generation += 1;
        refresh.cache.clear();
    }
}

#[tauri::command]
fn refresh_repository(
    repository_id: RepositoryId,
    state: State<'_, AppState>,
) -> Result<RepositorySummary, String> {
    let generation = start_summary_refresh(&state)?;
    let summary = state.git()?.summary(&state.record(repository_id)?);
    publish_summary_batch(&state, generation, std::slice::from_ref(&summary), |_| {});
    Ok(summary)
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
    clear_summary_cache(&state);
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
    replace_cached_summary(state, git.summary(&record))
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
    replace_cached_summary(&state, git.summary(&record))
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
    invalidate_summary_refresh(&state);
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
    invalidate_summary_refresh(&state);
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
    remove_cached_summary(&state, repository_id);
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
                conflicts: HashMap::new(),
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
fn get_conflict_document(
    repository_id: RepositoryId,
    snapshot_id: u64,
    path: String,
    state: State<'_, AppState>,
) -> Result<ConflictDocument, String> {
    validate_relative_path(&path)?;
    let git = state.git()?;
    let record = state.record(repository_id)?;
    let inspection = git.inspect_repository(Path::new(&record.path))?;
    if inspection.bare {
        return Err("Bare repositories do not have conflicts".into());
    }
    safe_worktree_file(&inspection.root, &path)?;
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
        .text(&inspection.root, &["rev-parse", "--verify", "HEAD"])
        .ok();
    if snapshot.head_oid != current_head {
        return Err("HEAD changed. Refresh the repository and try again.".into());
    }
    let source = git.conflict_source(&inspection.root, &path, snapshot_id)?;
    let document = source.document.clone();
    snapshot.conflicts.insert(document.id.clone(), source);
    Ok(document)
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

fn safe_worktree_file(root: &Path, path: &str) -> Result<PathBuf, String> {
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
        OperationRequest::TrashUntracked { .. }
        | OperationRequest::ResolveConflictBlocks { .. } => unreachable!(),
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
                summary_refresh: Mutex::new(SummaryRefreshState::default()),
                summary_refresh_running: Mutex::new(0),
                summary_refresh_ready: Condvar::new(),
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
            get_conflict_document,
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
    use std::process::Output;

    fn git_ok(git: &Git, cwd: &Path, args: &[&str]) {
        ensure_success(git.run(cwd, &strings(args), None).unwrap()).unwrap();
    }

    fn init_repo(git: &Git, path: &Path) {
        git_ok(git, path, &["init", "-b", "main"]);
        git_ok(git, path, &["config", "user.name", "GitDock Tests"]);
        git_ok(git, path, &["config", "user.email", "gitdock@example.com"]);
    }

    fn run_request(state: &AppState, cwd: &Path, request: OperationRequest) -> Output {
        let spec = command_spec(state, 1, cwd, &request).unwrap();
        let output = state
            .git()
            .unwrap()
            .run(cwd, &spec.args, spec.input.as_deref())
            .unwrap();
        ensure_success(output).unwrap()
    }

    fn commit_file(git: &Git, cwd: &Path, path: &str, contents: &str, message: &str) {
        fs::write(cwd.join(path), contents).unwrap();
        git_ok(git, cwd, &["add", path]);
        git_ok(git, cwd, &["commit", "-m", message]);
    }

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
            summary_refresh: Mutex::new(SummaryRefreshState::default()),
            summary_refresh_running: Mutex::new(0),
            summary_refresh_ready: Condvar::new(),
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
    fn cached_summary_uses_current_repository_metadata_and_generation() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repo(&git, dir.path());
        commit_file(&git, dir.path(), "a", "a", "base");
        let original = RepositoryRecord {
            id: 1,
            path: dir.path().to_string_lossy().into(),
            name: "old".into(),
            group: None,
            favorite: false,
            order: 0,
        };
        let summary = git.summary(&original);
        let current = RepositoryRecord {
            name: "new".into(),
            group: Some("work".into()),
            favorite: true,
            order: 49,
            ..original
        };
        let refreshed = summary_with_record(summary, &current);
        assert_eq!(refreshed.name, "new");
        assert_eq!(refreshed.group.as_deref(), Some("work"));
        assert!(refreshed.favorite);
        assert_eq!(refreshed.order, 49);

        let state = test_state(git, dir.path().join("config.json"));
        let generation = state.summary_refresh.lock().unwrap().generation;
        invalidate_summary_refresh(&state);
        assert_eq!(
            state.summary_refresh.lock().unwrap().generation,
            generation + 1
        );

        let stale_generation = start_summary_refresh(&state).unwrap();
        invalidate_summary_refresh(&state);
        let mut published = false;
        assert!(!publish_summary_batch(
            &state,
            stale_generation,
            std::slice::from_ref(&refreshed),
            |_| published = true,
        ));
        assert!(!published);
        assert!(state.summary_refresh.lock().unwrap().cache.is_empty());
    }

    #[test]
    fn background_summary_refresh_never_exceeds_four_slots() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(test_state(git, dir.path().join("config.json")));
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let maximum = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let workers = (0..8)
            .map(|_| {
                let state = state.clone();
                let barrier = barrier.clone();
                let active = active.clone();
                let maximum = maximum.clone();
                thread::spawn(move || {
                    barrier.wait();
                    let _permit = acquire_summary_refresh_permit(&state).unwrap();
                    let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(count, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(20));
                    active.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect::<Vec<_>>();
        workers
            .into_iter()
            .for_each(|worker| worker.join().unwrap());
        assert_eq!(maximum.load(Ordering::SeqCst), 4);
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
    #[ignore = "manual performance benchmark"]
    fn benchmarks_cached_fifty_repository_response() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repo(&git, dir.path());
        commit_file(&git, dir.path(), "a", "a\n", "base");
        let records = (0..50)
            .map(|id| RepositoryRecord {
                id,
                path: dir.path().to_string_lossy().into(),
                name: format!("repo-{id}"),
                group: None,
                favorite: false,
                order: id as u32,
            })
            .collect::<Vec<_>>();
        let cached = git.summary(&records[1]);
        let mut samples = Vec::new();
        for _ in 0..20 {
            let started = std::time::Instant::now();
            let active = git.summary(&records[0]);
            let summaries = std::iter::once(active)
                .chain(
                    records[1..]
                        .iter()
                        .map(|record| summary_with_record(cached.clone(), record)),
                )
                .collect::<Vec<_>>();
            assert_eq!(summaries.len(), 50);
            samples.push(started.elapsed());
        }
        samples.sort();
        eprintln!("50 repository cached refresh p95: {:?}", samples[18]);
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
