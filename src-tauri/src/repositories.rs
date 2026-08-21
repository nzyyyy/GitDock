use crate::{
    git::{ensure_success, path_name, strings, Git},
    models::*,
    process::{spawn_git_operation, CommandSpec, OperationContext},
    summary::{
        clear_summary_cache, invalidate_summary_refresh, remove_cached_summary,
        replace_cached_summary, repository_summary,
    },
    AppState, RepositoryChanged, RepositoryListChanged,
};
use notify::{RecursiveMode, Watcher};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{atomic::Ordering, mpsc},
    thread,
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, State};

#[tauri::command]
#[specta::specta]
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
        store.update(|config| {
            config.settings.git_path = path;
            Ok(())
        })?;
    }
    *state.git.lock().map_err(|_| "Git state is busy")? = discovered.clone();
    clear_summary_cache(&state);
    Ok(Git::info(&discovered))
}

#[tauri::command]
#[specta::specta]
pub(crate) fn save_layout(
    left_width: u16,
    right_width: u16,
    output_height: u16,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|_| "Settings are busy")?;
    store.update(|config| {
        config.settings.left_width = left_width.clamp(190, 420);
        config.settings.right_width = right_width.clamp(300, 560);
        config.settings.output_height = output_height.clamp(120, 420);
        Ok(())
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) fn save_language(language: Language, state: State<'_, AppState>) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|_| "Settings are busy")?;
    store.update(|config| {
        config.settings.language = language;
        Ok(())
    })
}

#[tauri::command]
#[specta::specta]
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
        store.update(|config| {
            if config.repositories.iter().any(|r| r.path == canonical) {
                return Err("This repository is already registered".into());
            }
            let id = config.next_repository_id;
            config.next_repository_id += 1;
            let record = RepositoryRecord {
                id,
                path: canonical,
                name: path_name(&inspection.root),
                group: None,
                favorite: false,
                order: config.repositories.len() as u32,
            };
            config.repositories.push(record.clone());
            config.settings.selected_repository_id = Some(id);
            Ok(record)
        })?
    };
    let _ = app.emit("repository-list-changed", RepositoryListChanged);
    let summary = replace_cached_summary(state, repository_summary(&git, &record))?;
    let _ = ensure_watch(record.id, PathBuf::from(&record.path), state, app);
    Ok(summary)
}

#[tauri::command]
#[specta::specta]
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
#[specta::specta]
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
            env: Vec::new(),
            cleanup_dir: None,
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
#[specta::specta]
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
        store.update(|config| {
            if config
                .repositories
                .iter()
                .any(|r| r.id != repository_id && r.path == canonical)
            {
                return Err("This repository is already registered".into());
            }
            let record = config
                .repositories
                .iter_mut()
                .find(|r| r.id == repository_id)
                .ok_or("Repository is not registered")?;
            record.path = canonical;
            record.name = path_name(&inspection.root);
            Ok(record.clone())
        })?
    };
    let _ = app.emit("repository-list-changed", RepositoryListChanged);
    let summary = replace_cached_summary(&state, repository_summary(&git, &record))?;
    let _ = ensure_watch(record.id, PathBuf::from(&record.path), &state, &app);
    Ok(summary)
}

#[tauri::command]
#[specta::specta]
pub(crate) fn update_repository(
    repository: RepositoryRecord,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|_| "Settings are busy")?;
    store.update(|config| {
        let existing = config
            .repositories
            .iter_mut()
            .find(|r| r.id == repository.id)
            .ok_or("Repository is not registered")?;
        existing.name = repository.name;
        existing.group = repository.group;
        existing.favorite = repository.favorite;
        existing.order = repository.order;
        Ok(())
    })?;
    invalidate_summary_refresh(&state);
    let _ = app.emit("repository-list-changed", RepositoryListChanged);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub(crate) fn reorder_repositories(
    placements: Vec<RepositoryPlacement>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|_| "Settings are busy")?;
    store.update(|config| apply_repository_placements(&mut config.repositories, &placements))?;
    invalidate_summary_refresh(&state);
    let _ = app.emit("repository-list-changed", RepositoryListChanged);
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

fn sanitize_group_order(groups: Vec<String>) -> Result<Vec<String>, String> {
    let mut seen = HashSet::new();
    let sanitized = groups
        .into_iter()
        .map(|group| group.trim().to_string())
        .filter(|group| !group.is_empty() && seen.insert(group.clone()))
        .collect::<Vec<_>>();
    if sanitized
        .iter()
        .any(|group| group.len() > 100 || group.chars().any(char::is_control))
    {
        return Err("Repository group must be at most 100 characters".into());
    }
    Ok(sanitized)
}

#[tauri::command]
#[specta::specta]
pub(crate) fn save_group_order(
    groups: Vec<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let groups = sanitize_group_order(groups)?;
    let mut store = state.store.lock().map_err(|_| "Settings are busy")?;
    store.update(|config| {
        config.settings.group_order = groups;
        Ok(())
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) fn remove_repository(
    repository_id: RepositoryId,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|_| "Settings are busy")?;
    store.update(|config| {
        config.repositories.retain(|r| r.id != repository_id);
        if config.settings.selected_repository_id == Some(repository_id) {
            config.settings.selected_repository_id = None;
        }
        Ok(())
    })?;
    remove_cached_summary(&state, repository_id);
    stop_watch(repository_id, &state);
    let _ = app.emit("repository-list-changed", RepositoryListChanged);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub(crate) fn watch_repository(
    repository_id: RepositoryId,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let record = state.record(repository_id)?;
    ensure_watch(repository_id, PathBuf::from(record.path), &state, &app)
}

pub(crate) fn ensure_watch(
    repository_id: RepositoryId,
    path: PathBuf,
    state: &AppState,
    app: &AppHandle,
) -> Result<(), String> {
    if !path.exists() {
        stop_watch(repository_id, state);
        return Ok(());
    }
    let mut routes = state
        .watch_routes
        .lock()
        .map_err(|_| "File watcher is busy")?;
    if routes
        .get(&repository_id)
        .is_some_and(|(existing, _)| existing == &path)
    {
        return Ok(());
    }
    let old_path = routes.remove(&repository_id).map(|(path, _)| path);
    let (events, pending) = mpsc::channel();
    routes.insert(repository_id, (path.clone(), events));
    drop(routes);

    let event_app = app.clone();
    thread::spawn(move || {
        while wait_for_quiet(&pending) {
            let _ = event_app.emit("repository-changed", RepositoryChanged { repository_id });
        }
    });

    let mut watcher_slot = state.watcher.lock().map_err(|_| "File watcher is busy")?;
    if watcher_slot.is_none() {
        let watch_app = app.clone();
        *watcher_slot = Some(
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                dispatch_watch_event(&watch_app, event);
            })
            .map_err(|e| e.to_string())?,
        );
    }
    let watcher = watcher_slot.as_mut().ok_or("File watcher is unavailable")?;
    if let Some(old_path) = old_path {
        let _ = watcher.unwatch(&old_path);
    }
    watcher
        .watch(&path, RecursiveMode::Recursive)
        .map_err(|e| e.to_string())
}

fn stop_watch(repository_id: RepositoryId, state: &AppState) {
    let Ok(mut routes) = state.watch_routes.lock() else {
        return;
    };
    let Some((path, _)) = routes.remove(&repository_id) else {
        return;
    };
    drop(routes);
    if let Ok(mut watcher) = state.watcher.lock() {
        if let Some(watcher) = watcher.as_mut() {
            let _ = watcher.unwatch(&path);
        }
    }
}

fn dispatch_watch_event(app: &AppHandle, event: notify::Result<notify::Event>) {
    let Ok(event) = event else {
        return;
    };
    let state = app.state::<AppState>();
    let Ok(mutating) = state.mutating_repositories.lock() else {
        return;
    };
    let Ok(routes) = state.watch_routes.lock() else {
        return;
    };
    let mut notified = HashSet::new();
    for path in &event.paths {
        let Some(repository_id) = match_watch_path(
            routes.iter().map(|(id, (root, _))| (*id, root.as_path())),
            path,
        ) else {
            continue;
        };
        if mutating.contains(&repository_id) || !notified.insert(repository_id) {
            continue;
        }
        if let Some((_, sender)) = routes.get(&repository_id) {
            let _ = sender.send(());
        }
    }
}

fn match_watch_path<'a>(
    paths: impl IntoIterator<Item = (RepositoryId, &'a Path)>,
    event_path: &Path,
) -> Option<RepositoryId> {
    paths
        .into_iter()
        .filter(|(_, root)| event_path.starts_with(root))
        .max_by_key(|(_, root)| root.as_os_str().len())
        .map(|(id, _)| id)
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
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
    };

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
    fn watching_a_second_repository_keeps_the_first_route() {
        let paths = HashMap::from([(1, PathBuf::from("/alpha")), (2, PathBuf::from("/beta"))]);
        assert_eq!(
            match_watch_path(
                paths.iter().map(|(id, path)| (*id, path.as_path())),
                Path::new("/alpha/src/a.ts"),
            ),
            Some(1)
        );
        assert_eq!(
            match_watch_path(
                paths.iter().map(|(id, path)| (*id, path.as_path())),
                Path::new("/beta/src/b.ts"),
            ),
            Some(2)
        );
    }

    #[test]
    fn nested_repository_prefers_the_longest_path_prefix() {
        let paths = HashMap::from([
            (1, PathBuf::from("/work/app")),
            (2, PathBuf::from("/work/app/vendor/nested")),
        ]);
        assert_eq!(
            match_watch_path(
                paths.iter().map(|(id, path)| (*id, path.as_path())),
                Path::new("/work/app/src/main.rs"),
            ),
            Some(1)
        );
        assert_eq!(
            match_watch_path(
                paths.iter().map(|(id, path)| (*id, path.as_path())),
                Path::new("/work/app/vendor/nested/lib.rs"),
            ),
            Some(2)
        );
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
    fn group_order_is_sanitized_and_validated() {
        assert_eq!(
            sanitize_group_order(vec![
                " Work ".into(),
                "".into(),
                "  ".into(),
                "Work".into(),
                "Team".into(),
                "Team".into(),
            ])
            .unwrap(),
            vec!["Work", "Team"]
        );
        let mut long = "x".repeat(101);
        assert!(sanitize_group_order(vec![long.clone()]).is_err());
        long.truncate(100);
        assert!(sanitize_group_order(vec![long]).is_ok());
        assert!(sanitize_group_order(vec!["bad\u{1}name".into()]).is_err());
    }

    #[test]
    fn group_order_persists_through_the_config_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut store = ConfigStore::load(path.clone()).unwrap();
        store
            .update(|config| {
                config.settings.group_order = sanitize_group_order(vec!["Work".into()]).unwrap();
                Ok(())
            })
            .unwrap();
        let reloaded = ConfigStore::load(path).unwrap();
        assert_eq!(reloaded.config().settings.group_order, vec!["Work"]);
    }
}
