use crate::{models::*, snapshot::cache_snapshot, AppState};
use std::{collections::HashMap, sync::atomic::Ordering, thread};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Default)]
pub(crate) struct SummaryRefreshState {
    pub(crate) generation: u64,
    pub(crate) cache: HashMap<RepositoryId, RepositorySummary>,
}

pub(crate) struct SummaryRefreshPermit<'a>(&'a AppState);

impl Drop for SummaryRefreshPermit<'_> {
    fn drop(&mut self) {
        if let Ok(mut running) = self.0.summary_refresh_running.lock() {
            *running -= 1;
            self.0.summary_refresh_ready.notify_one();
        }
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) fn refresh_repositories(
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

pub(crate) fn summary_with_record(
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

pub(crate) fn start_summary_refresh(state: &AppState) -> Result<u64, String> {
    let mut refresh = state
        .summary_refresh
        .lock()
        .map_err(|_| "Repository summary cache is busy")?;
    refresh.generation += 1;
    Ok(refresh.generation)
}

pub(crate) fn invalidate_summary_refresh(state: &AppState) {
    if let Ok(mut refresh) = state.summary_refresh.lock() {
        refresh.generation += 1;
    }
}

pub(crate) fn summary_refresh_is_current(state: &AppState, generation: u64) -> bool {
    state
        .summary_refresh
        .lock()
        .is_ok_and(|refresh| refresh.generation == generation)
}

pub(crate) fn publish_summary_batch(
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

pub(crate) fn acquire_summary_refresh_permit(
    state: &AppState,
) -> Result<SummaryRefreshPermit<'_>, String> {
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

pub(crate) fn replace_cached_summary(
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

pub(crate) fn remove_cached_summary(state: &AppState, repository_id: RepositoryId) {
    if let Ok(mut refresh) = state.summary_refresh.lock() {
        refresh.generation += 1;
        refresh.cache.remove(&repository_id);
    }
}

pub(crate) fn clear_summary_cache(state: &AppState) {
    if let Ok(mut refresh) = state.summary_refresh.lock() {
        refresh.generation += 1;
        refresh.cache.clear();
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) fn refresh_repository(
    repository_id: RepositoryId,
    state: State<'_, AppState>,
) -> Result<RepositoryRefresh, String> {
    let generation = start_summary_refresh(&state)?;
    let snapshot_id = state.next_snapshot_id.fetch_add(1, Ordering::Relaxed);
    let (summary, snapshot) = state
        .git()?
        .summary_with_snapshot(&state.record(repository_id)?, snapshot_id);
    if let Some(snapshot) = &snapshot {
        cache_snapshot(&state, snapshot)?;
    }
    publish_summary_batch(&state, generation, std::slice::from_ref(&summary), |_| {});
    Ok(RepositoryRefresh { summary, snapshot })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        git::Git,
        test_util::{commit_file, init_repo, test_state},
    };
    use std::{
        sync::{atomic::AtomicUsize, Arc, Barrier},
        time::Duration,
    };

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
        let barrier = Arc::new(Barrier::new(8));
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
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
}
