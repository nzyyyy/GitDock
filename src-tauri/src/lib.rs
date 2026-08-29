mod git;
mod history;
mod models;
mod operations;
mod process;
mod repositories;
mod repository_path;
mod store;
mod summary;
mod working_tree;

use crate::{git::Git, models::*, store::ConfigStore, summary::SummaryRefreshState};
use notify::RecommendedWatcher;
use serde::Serialize;
#[cfg(any(debug_assertions, test))]
use specta_typescript::Typescript;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{atomic::AtomicU64, Condvar, Mutex},
};
use tauri::{AppHandle, Manager, State};
use tauri_specta::{collect_commands, Builder, ErrorHandlingMode};

pub struct AppState {
    pub(crate) store: Mutex<ConfigStore>,
    pub(crate) git: Mutex<Result<Git, String>>,
    pub(crate) working_tree: Mutex<working_tree::Cache>,
    pub(crate) next_snapshot_id: AtomicU64,
    pub(crate) next_operation_id: AtomicU64,
    pub(crate) write_locks: Mutex<HashSet<PathBuf>>,
    pub(crate) mutating_repositories: Mutex<HashSet<RepositoryId>>,
    pub(crate) running: Mutex<HashMap<OperationId, process::RunningOperation>>,
    pub(crate) watcher: Mutex<Option<RecommendedWatcher>>,
    pub(crate) watch_routes: Mutex<HashMap<RepositoryId, (PathBuf, std::sync::mpsc::Sender<()>)>>,
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
            .config()
            .repositories
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| "Repository is not registered".into())
    }
}

#[tauri::command]
#[specta::specta]
fn bootstrap(state: State<'_, AppState>, app: AppHandle) -> Result<Bootstrap, String> {
    let (settings, records) = {
        let store = state.store.lock().map_err(|_| "Settings are busy")?;
        (
            store.config().settings.clone(),
            store.config().repositories.clone(),
        )
    };
    let git = state.git.lock().map_err(|_| "Git state is busy")?.clone();
    let repositories: Vec<RepositorySummary> = git
        .as_ref()
        .map(|git| {
            records
                .iter()
                .map(|record| summary::repository_summary(git, record))
                .collect()
        })
        .unwrap_or_default();
    state
        .summary_refresh
        .lock()
        .map_err(|_| "Repository summary cache is busy")?
        .cache = repositories
        .iter()
        .map(|summary| (summary.id, summary.clone()))
        .collect();
    for record in &records {
        let _ = repositories::ensure_watch(record.id, PathBuf::from(&record.path), &state, &app);
    }
    Ok(Bootstrap {
        git: Git::info(&git),
        settings,
        repositories,
    })
}

fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            bootstrap,
            summary::refresh_repositories,
            summary::refresh_repository,
            repositories::set_git_path,
            repositories::save_layout,
            repositories::save_language,
            repositories::add_repository,
            repositories::initialize_repository,
            repositories::clone_repository,
            repositories::relocate_repository,
            repositories::update_repository,
            repositories::reorder_repositories,
            repositories::save_group_order,
            repositories::remove_repository,
            repositories::watch_repository,
            working_tree::get_status,
            working_tree::get_diff,
            working_tree::get_conflict_document,
            history::get_history,
            history::export_session_log,
            history::get_commit_detail,
            history::get_stash_detail,
            history::compare_branches,
            history::open_repository_file,
            history::get_branches,
            history::get_tags,
            history::get_remotes,
            history::get_stashes,
            history::get_submodules,
            history::get_rebase_commits,
            history::get_file_history,
            history::get_commit_file_diff,
            history::get_stash_file_diff,
            history::get_blame,
            operations::preview_operation,
            operations::start_operation,
            operations::cancel_operation
        ])
        .typ::<OperationEvent>()
        .error_handling(ErrorHandlingMode::Throw)
        .dangerously_cast_bigints_to_number()
}

#[cfg(any(debug_assertions, test))]
fn bindings_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/bindings.ts")
}

pub fn run() {
    let builder = specta_builder();
    #[cfg(debug_assertions)]
    builder
        .export(Typescript::default(), bindings_path())
        .expect("failed to export TypeScript bindings");

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
            let git = Git::discover(store.config().settings.git_path.as_deref());
            app.manage(AppState {
                store: Mutex::new(store),
                git: Mutex::new(git),
                working_tree: Mutex::new(working_tree::Cache::default()),
                next_snapshot_id: AtomicU64::new(1),
                next_operation_id: AtomicU64::new(1),
                write_locks: Mutex::new(HashSet::new()),
                mutating_repositories: Mutex::new(HashSet::new()),
                running: Mutex::new(HashMap::new()),
                watcher: Mutex::new(None),
                watch_routes: Mutex::new(HashMap::new()),
                summary_refresh: Mutex::new(SummaryRefreshState::default()),
                summary_refresh_running: Mutex::new(0),
                summary_refresh_ready: Condvar::new(),
            });
            Ok(())
        })
        .invoke_handler(builder.invoke_handler())
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
            working_tree: Mutex::new(working_tree::Cache::default()),
            next_snapshot_id: AtomicU64::new(1),
            next_operation_id: AtomicU64::new(1),
            write_locks: Mutex::new(HashSet::new()),
            mutating_repositories: Mutex::new(HashSet::new()),
            running: Mutex::new(HashMap::new()),
            watcher: Mutex::new(None),
            watch_routes: Mutex::new(HashMap::new()),
            summary_refresh: Mutex::new(SummaryRefreshState::default()),
            summary_refresh_running: Mutex::new(0),
            summary_refresh_ready: Condvar::new(),
        }
    }
}

#[cfg(test)]
mod binding_tests {
    use super::*;

    #[test]
    fn generated_bindings_are_current() {
        if std::env::var_os("GITDOCK_UPDATE_BINDINGS").is_some() {
            specta_builder()
                .export(Typescript::default(), bindings_path())
                .unwrap();
            return;
        }
        let output = tempfile::NamedTempFile::new().unwrap();
        specta_builder()
            .export(Typescript::default(), output.path())
            .unwrap();
        assert_eq!(
            std::fs::read(output.path()).unwrap(),
            std::fs::read(bindings_path()).unwrap(),
            "bindings are stale; run the app in debug mode to regenerate them"
        );
    }
}
