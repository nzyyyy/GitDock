use crate::{
    git::{ensure_success, path_name, strings, Git},
    models::*,
    process::{spawn_git_operation, CommandSpec, OperationContext},
    store::ConfigStore,
    summary::{
        clear_summary_cache, invalidate_summary_refresh, remove_cached_summary,
        replace_cached_summary,
    },
    AppState, RepositoryChanged, RepositoryListChanged,
};
use notify::{RecursiveMode, Watcher};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{atomic::Ordering, mpsc},
    thread,
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, State};

#[tauri::command]
pub(crate) fn set_git_path(
    path: Option<String>,
    state: State<'_, AppState>,
) -> Result<GitInfo, String> {
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
pub(crate) fn save_layout(
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
pub(crate) fn save_language(language: Language, state: State<'_, AppState>) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|_| "Settings are busy")?;
    store.config.settings.language = language;
    store.save()
}

#[tauri::command]
pub(crate) fn add_repository(
    path: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<RepositorySummary, String> {
    register_repository(path, &state, &app)
}

pub(crate) fn register_repository(
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
pub(crate) fn initialize_repository(
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
pub(crate) fn clone_repository(
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
pub(crate) fn relocate_repository(
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
pub(crate) fn update_repository(
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
pub(crate) fn reorder_repositories(
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

pub(crate) fn persist_repository_placements(
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
pub(crate) fn remove_repository(
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
pub(crate) fn watch_repository(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ConfigStore;
    use std::fs;

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
}
