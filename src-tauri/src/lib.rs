mod git;
mod history;
mod models;
mod operations;
mod process;
mod repositories;
mod snapshot;
mod store;
mod summary;

use crate::{
    git::Git,
    history::{
        compare_branches, export_session_log, get_blame, get_branches, get_commit_diff,
        get_commit_file_diff, get_file_history, get_history, get_rebase_commits, get_remotes,
        get_stashes, get_submodules, get_tags, open_repository_file,
    },
    models::*,
    operations::{cancel_operation, preview_operation, start_operation},
    repositories::{
        add_repository, clone_repository, initialize_repository, relocate_repository,
        remove_repository, reorder_repositories, save_group_order, save_language, save_layout,
        set_git_path, update_repository, watch_repository,
    },
    snapshot::{get_conflict_document, get_diff, get_status},
    store::ConfigStore,
    summary::{refresh_repositories, refresh_repository, SummaryRefreshState},
};
use notify::RecommendedWatcher;
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{atomic::AtomicU64, Condvar, Mutex},
};
use tauri::{Manager, State};

pub struct AppState {
    pub(crate) store: Mutex<ConfigStore>,
    pub(crate) git: Mutex<Result<Git, String>>,
    pub(crate) snapshots: Mutex<HashMap<u64, snapshot::SnapshotCache>>,
    pub(crate) next_snapshot_id: AtomicU64,
    pub(crate) next_operation_id: AtomicU64,
    pub(crate) write_locks: Mutex<HashSet<PathBuf>>,
    pub(crate) mutating_repositories: Mutex<HashSet<RepositoryId>>,
    pub(crate) running: Mutex<HashMap<OperationId, process::RunningOperation>>,
    pub(crate) watcher: Mutex<Option<RecommendedWatcher>>,
    pub(crate) summary_refresh: Mutex<SummaryRefreshState>,
    pub(crate) summary_refresh_running: Mutex<usize>,
    pub(crate) summary_refresh_ready: Condvar,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepositoryChanged {
    pub(crate) repository_id: RepositoryId,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepositoryListChanged;

impl AppState {
    pub(crate) fn git(&self) -> Result<Git, String> {
        self.git
            .lock()
            .map_err(|_| "Git state is unavailable".to_string())?
            .clone()
    }

    pub(crate) fn record(&self, id: RepositoryId) -> Result<RepositoryRecord, String> {
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
            save_group_order,
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
            get_rebase_commits,
            get_file_history,
            get_commit_file_diff,
            get_blame,
            preview_operation,
            start_operation,
            cancel_operation
        ])
        .run(tauri::generate_context!())
        .expect("error while running GitDock");
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;
    use crate::git::{ensure_success, strings};
    use std::{fs, path::Path, process::Output};

    pub(crate) fn git_ok(git: &Git, cwd: &Path, args: &[&str]) {
        ensure_success(git.run(cwd, &strings(args), None).unwrap()).unwrap();
    }

    pub(crate) fn init_repo(git: &Git, path: &Path) {
        git_ok(git, path, &["init", "-b", "main"]);
        git_ok(git, path, &["config", "user.name", "GitDock Tests"]);
        git_ok(git, path, &["config", "user.email", "gitdock@example.com"]);
    }

    pub(crate) fn run_request(state: &AppState, cwd: &Path, request: OperationRequest) -> Output {
        let spec = operations::command_spec(state, 1, cwd, &request).unwrap();
        let output = state
            .git()
            .unwrap()
            .run_env(cwd, &spec.args, spec.input.as_deref(), &spec.env)
            .unwrap();
        ensure_success(output).unwrap()
    }

    pub(crate) fn commit_file(git: &Git, cwd: &Path, path: &str, contents: &str, message: &str) {
        fs::write(cwd.join(path), contents).unwrap();
        git_ok(git, cwd, &["add", path]);
        git_ok(git, cwd, &["commit", "-m", message]);
    }

    pub(crate) fn test_state(git: Git, config_path: PathBuf) -> AppState {
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
}
