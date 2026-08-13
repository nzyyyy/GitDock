use crate::{
    git::ConflictSource,
    models::*,
    operations::{safe_worktree_file, validate_relative_path},
    AppState,
};
use std::{collections::HashMap, path::Path, sync::atomic::Ordering};
use tauri::State;

#[derive(Clone)]
pub(crate) struct SnapshotCache {
    pub(crate) repository_id: RepositoryId,
    pub(crate) head_oid: Option<String>,
    pub(crate) hunks: HashMap<String, CachedHunk>,
    pub(crate) conflicts: HashMap<String, ConflictSource>,
}

#[derive(Clone)]
pub(crate) struct CachedHunk {
    pub(crate) path: String,
    pub(crate) staged: bool,
    pub(crate) patch: Vec<u8>,
    pub(crate) source_diff: String,
}

#[tauri::command]
pub(crate) fn get_status(
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
    cache_snapshot(&state, &snapshot)?;
    Ok(snapshot)
}

pub(crate) fn cache_snapshot(
    state: &AppState,
    snapshot: &WorkingTreeSnapshot,
) -> Result<(), String> {
    state
        .snapshots
        .lock()
        .map_err(|_| "Snapshot cache is busy")?
        .insert(
            snapshot.id,
            SnapshotCache {
                repository_id: snapshot.repository_id,
                head_oid: snapshot.head_oid.clone(),
                hunks: HashMap::new(),
                conflicts: HashMap::new(),
            },
        );
    Ok(())
}

#[tauri::command]
pub(crate) fn get_diff(
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
pub(crate) fn get_conflict_document(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{git::Git, test_util::test_state};

    #[test]
    fn caches_combined_refresh_snapshots() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(git, dir.path().join("config.json"));
        let snapshot = WorkingTreeSnapshot {
            id: 9,
            repository_id: 3,
            head_oid: Some("head".into()),
            files: Vec::new(),
        };
        cache_snapshot(&state, &snapshot).unwrap();
        let snapshots = state.snapshots.lock().unwrap();
        let cached = snapshots.get(&9).unwrap();
        assert_eq!(cached.repository_id, 3);
        assert_eq!(cached.head_oid.as_deref(), Some("head"));
    }
}
