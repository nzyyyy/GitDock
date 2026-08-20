mod conflict;
mod diff;

use crate::{git::Git, models::*, repository_path::validate_relative_path, AppState};
use std::{collections::HashMap, path::Path, sync::atomic::Ordering};
use tauri::State;

#[derive(Clone)]
struct SnapshotCache {
    repository_id: RepositoryId,
    head_oid: Option<String>,
    hunks: HashMap<String, CachedHunk>,
    conflicts: HashMap<String, conflict::ConflictSource>,
}

#[derive(Clone)]
struct CachedHunk {
    path: String,
    staged: bool,
    patch: Vec<u8>,
    source_diff: String,
}

#[derive(Default)]
pub(crate) struct Cache {
    snapshots: HashMap<u64, SnapshotCache>,
}

#[tauri::command]
#[specta::specta]
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
    let mut snapshot = read_snapshot(&git, repository_id, &inspection.root, include_ignored, id)?;
    attach_line_stats(&git, &inspection.root, &mut snapshot.files);
    cache_snapshot(&state, &snapshot)?;
    Ok(snapshot)
}

pub(crate) fn read_snapshot(
    git: &Git,
    repository_id: RepositoryId,
    root: &Path,
    include_ignored: bool,
    snapshot_id: u64,
) -> Result<WorkingTreeSnapshot, String> {
    diff::status(git, repository_id, root, include_ignored, snapshot_id)
}

pub(crate) fn attach_line_stats(git: &Git, root: &Path, files: &mut [FileChange]) {
    diff::attach_line_stats(git, root, files);
}

pub(crate) fn read_diff(
    git: &Git,
    root: &Path,
    path: &str,
    staged: bool,
    snapshot_id: u64,
) -> Result<DiffFile, String> {
    diff::diff(git, root, path, staged, snapshot_id)
}

pub(crate) fn cache_snapshot(
    state: &AppState,
    snapshot: &WorkingTreeSnapshot,
) -> Result<(), String> {
    state
        .working_tree
        .lock()
        .map_err(|_| "Snapshot cache is busy")?
        .snapshots
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
#[specta::specta]
pub(crate) fn get_diff(
    repository_id: RepositoryId,
    snapshot_id: u64,
    path: String,
    staged: bool,
    state: State<'_, AppState>,
) -> Result<DiffFile, String> {
    let git = state.git()?;
    let record = state.record(repository_id)?;
    load_diff(
        &git,
        Path::new(&record.path),
        &state,
        repository_id,
        snapshot_id,
        &path,
        staged,
    )
}

pub(crate) fn load_diff(
    git: &Git,
    root: &Path,
    state: &AppState,
    repository_id: RepositoryId,
    snapshot_id: u64,
    path: &str,
    staged: bool,
) -> Result<DiffFile, String> {
    validate_relative_path(path)?;
    let mut cache = state
        .working_tree
        .lock()
        .map_err(|_| "Snapshot cache is busy")?;
    let snapshot = cache
        .snapshots
        .get_mut(&snapshot_id)
        .ok_or("This view is stale. Refresh the repository and try again.")?;
    if snapshot.repository_id != repository_id {
        return Err("Snapshot does not belong to this repository".into());
    }
    let current_head = git.text(root, &["rev-parse", "--verify", "HEAD"]).ok();
    if snapshot.head_oid != current_head {
        return Err("HEAD changed. Refresh the repository and try again.".into());
    }
    let diff = read_diff(git, root, path, staged, snapshot_id)?;
    for hunk in &diff.hunks {
        snapshot.hunks.insert(
            hunk.id.clone(),
            CachedHunk {
                path: path.into(),
                staged,
                patch: hunk.patch.as_bytes().to_vec(),
                source_diff: diff.patch.clone(),
            },
        );
    }
    Ok(diff)
}

#[tauri::command]
#[specta::specta]
pub(crate) fn get_conflict_document(
    repository_id: RepositoryId,
    snapshot_id: u64,
    path: String,
    state: State<'_, AppState>,
) -> Result<ConflictDocument, String> {
    let git = state.git()?;
    let record = state.record(repository_id)?;
    let inspection = git.inspect_repository(Path::new(&record.path))?;
    if inspection.bare {
        return Err("Bare repositories do not have conflicts".into());
    }
    load_conflict_document(
        &git,
        &inspection.root,
        &state,
        repository_id,
        snapshot_id,
        &path,
    )
}

pub(crate) fn load_conflict_document(
    git: &Git,
    root: &Path,
    state: &AppState,
    repository_id: RepositoryId,
    snapshot_id: u64,
    path: &str,
) -> Result<ConflictDocument, String> {
    validate_relative_path(path)?;
    conflict::safe_worktree_file(root, path)?;
    let mut cache = state
        .working_tree
        .lock()
        .map_err(|_| "Snapshot cache is busy")?;
    let snapshot = cache
        .snapshots
        .get_mut(&snapshot_id)
        .ok_or("This view is stale. Refresh the repository and try again.")?;
    if snapshot.repository_id != repository_id {
        return Err("Snapshot does not belong to this repository".into());
    }
    let current_head = git.text(root, &["rev-parse", "--verify", "HEAD"]).ok();
    if snapshot.head_oid != current_head {
        return Err("HEAD changed. Refresh the repository and try again.".into());
    }
    let source = conflict::source(git, root, path, snapshot_id)?;
    let document = source.document.clone();
    snapshot.conflicts.insert(document.id.clone(), source);
    Ok(document)
}

pub(crate) fn cached_hunk(
    git: &Git,
    cwd: &Path,
    state: &AppState,
    repository_id: RepositoryId,
    snapshot_id: u64,
    hunk_id: &str,
    worktree_only: bool,
) -> Result<Vec<u8>, String> {
    let cache = state
        .working_tree
        .lock()
        .map_err(|_| "Snapshot cache is busy")?;
    let snapshot = cache
        .snapshots
        .get(&snapshot_id)
        .ok_or("This diff is stale. Refresh and try again.")?;
    if snapshot.repository_id != repository_id {
        return Err("Snapshot does not belong to this repository".into());
    }
    let hunk = snapshot
        .hunks
        .get(hunk_id)
        .ok_or("Hunk is not available in this snapshot")?;
    if worktree_only && hunk.staged {
        return Err("Only unstaged hunks can be discarded".into());
    }
    let current = diff::diff(git, cwd, &hunk.path, hunk.staged, snapshot_id)?;
    if current.patch != hunk.source_diff {
        return Err("This diff changed after it was displayed. Refresh and try again.".into());
    }
    Ok(hunk.patch.clone())
}

pub(crate) fn resolve_conflict_blocks(
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
    let source = {
        let cache = state
            .working_tree
            .lock()
            .map_err(|_| "Snapshot cache is busy")?;
        let snapshot = cache
            .snapshots
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
    conflict::resolve(git, root, path, &source, choices)
}

pub(crate) fn invalidate_repository(state: &AppState, repository_id: RepositoryId) {
    if let Ok(mut cache) = state.working_tree.lock() {
        cache
            .snapshots
            .retain(|_, snapshot| snapshot.repository_id != repository_id);
    }
}

#[cfg(test)]
fn parse_conflict_segments_for_test(
    snapshot_id: u64,
    path: &str,
    merged: &str,
    labels: &[String; 3],
    originals: [&str; 3],
) -> Result<Vec<ConflictSegment>, String> {
    conflict::parse_segments(snapshot_id, path, merged, labels, originals)
}

#[cfg(test)]
fn render_conflict_resolution_for_test(
    segments: &[ConflictSegment],
    choices: &[ConflictResolution],
) -> Result<Vec<u8>, String> {
    conflict::render_resolution(segments, choices)
}

#[cfg(test)]
fn validate_conflict_text_for_test(content: &[u8]) -> Result<(), String> {
    conflict::validate_text(content)
}

#[cfg(test)]
fn read_conflict_document_for_test(
    git: &Git,
    root: &Path,
    path: &str,
    snapshot_id: u64,
) -> Result<ConflictDocument, String> {
    conflict::source(git, root, path, snapshot_id).map(|source| source.document)
}

#[cfg(test)]
fn parse_status_for_test(bytes: &[u8]) -> Vec<FileChange> {
    diff::parse_porcelain_v2(bytes)
}

#[cfg(test)]
fn split_hunks_for_test(snapshot_id: u64, path: &str, staged: bool, patch: &str) -> Vec<DiffHunk> {
    diff::split_hunks(snapshot_id, path, staged, patch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        git::{ensure_success, strings, Git, MAX_DIFF_BYTES},
        test_util::{commit_file, git_ok, init_repo, test_state},
    };
    use std::fs;

    #[test]
    fn snapshot_repository_ownership_is_enforced_through_the_interface() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repo(&git, dir.path());
        commit_file(&git, dir.path(), "file.txt", "base\n", "base");
        let state = test_state(git, dir.path().join("config.json"));
        let git = state.git().unwrap();
        let snapshot = read_snapshot(&git, 3, dir.path(), false, 9).unwrap();
        cache_snapshot(&state, &snapshot).unwrap();
        assert!(load_diff(&git, dir.path(), &state, 4, 9, "file.txt", false)
            .unwrap_err()
            .contains("does not belong"));
    }

    #[test]
    fn parses_zero_delimited_status_and_rename() {
        let input = b"1 M. N... 100644 100644 100644 a a src/a.rs\0? new file.txt\02 R. N... 100644 100644 100644 a a R100 new.rs\0old.rs\0! target/a\0";
        let files = parse_status_for_test(input);
        assert_eq!(files.len(), 4);
        assert!(files[0].staged);
        assert_eq!(files[1].kind, ChangeKind::Untracked);
        assert_eq!(files[2].original_path.as_deref(), Some("old.rs"));
        assert!(files[3].ignored);
    }

    #[test]
    fn reads_status_line_stats_and_untracked_diffs() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repo(&git, dir.path());
        commit_file(&git, dir.path(), "file.txt", "one\ntwo\n", "initial");
        fs::write(dir.path().join("file.txt"), "one\ntwo\nthree\n").unwrap();
        fs::write(dir.path().join("new.txt"), "alpha\nbeta\n").unwrap();
        let mut snapshot = read_snapshot(&git, 1, dir.path(), false, 1).unwrap();
        attach_line_stats(&git, dir.path(), &mut snapshot.files);
        let modified = snapshot
            .files
            .iter()
            .find(|file| file.path == "file.txt")
            .unwrap();
        let untracked = snapshot
            .files
            .iter()
            .find(|file| file.path == "new.txt")
            .unwrap();
        assert_eq!((modified.additions, modified.deletions), (Some(1), Some(0)));
        assert_eq!(
            (untracked.additions, untracked.deletions),
            (Some(2), Some(0))
        );
        let diff = read_diff(&git, dir.path(), "new.txt", false, 1).unwrap();
        assert!(diff.patch.contains("+alpha"));
        assert!(!diff.hunks.is_empty());
    }

    #[test]
    fn splits_git_hunks_into_backend_owned_change_islands() {
        let patch = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,5 +1,5 @@\n context\n-oldA\n+newA\n mid\n-oldB\n+newB\n";
        let hunks = split_hunks_for_test(1, "a", false, patch);
        assert_eq!(hunks.len(), 2);
        assert!(hunks[0].patch.contains("-oldA"));
        assert!(!hunks[0].patch.contains("-oldB"));
        assert!(hunks[1].patch.contains("-oldB"));
        assert_ne!(hunks[0].id, hunks[1].id);
    }

    #[test]
    fn applies_one_change_island_without_staging_the_other() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repo(&git, dir.path());
        commit_file(
            &git,
            dir.path(),
            "file.txt",
            "keep\noldA\nmid\noldB\nend\n",
            "base",
        );
        fs::write(dir.path().join("file.txt"), "keep\nnewA\nmid\nnewB\nend\n").unwrap();
        let diff = read_diff(&git, dir.path(), "file.txt", false, 1).unwrap();
        assert_eq!(diff.hunks.len(), 2);
        ensure_success(
            git.run(
                dir.path(),
                &strings(&["apply", "--cached", "--whitespace=nowarn", "-"]),
                Some(diff.hunks[0].patch.as_bytes()),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            git.text(dir.path(), &["show", ":file.txt"]).unwrap(),
            "keep\nnewA\nmid\noldB\nend"
        );
    }

    #[test]
    fn rejects_changed_head_and_staged_hunk_discard() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repo(&git, dir.path());
        commit_file(&git, dir.path(), "file.txt", "base\n", "base");
        fs::write(dir.path().join("file.txt"), "changed\n").unwrap();
        let state = test_state(git.clone(), dir.path().join("config.json"));

        let old_snapshot = read_snapshot(&git, 1, dir.path(), false, 10).unwrap();
        cache_snapshot(&state, &old_snapshot).unwrap();
        git_ok(&git, dir.path(), &["add", "file.txt"]);
        git_ok(&git, dir.path(), &["commit", "-m", "new head"]);
        assert!(
            load_diff(&git, dir.path(), &state, 1, 10, "file.txt", false)
                .unwrap_err()
                .contains("HEAD changed")
        );

        fs::write(dir.path().join("file.txt"), "staged\n").unwrap();
        git_ok(&git, dir.path(), &["add", "file.txt"]);
        let snapshot = read_snapshot(&git, 1, dir.path(), false, 11).unwrap();
        cache_snapshot(&state, &snapshot).unwrap();
        let staged = load_diff(&git, dir.path(), &state, 1, 11, "file.txt", true).unwrap();
        assert!(
            cached_hunk(&git, dir.path(), &state, 1, 11, &staged.hunks[0].id, true,)
                .unwrap_err()
                .contains("Only unstaged hunks")
        );
    }

    #[test]
    fn parses_and_resolves_multiple_conflict_blocks() {
        let labels = ["current".into(), "base".into(), "incoming".into()];
        let merged = "before\n<<<<<<< current\ncurrent one\n||||||| base\nbase one\n=======\nincoming one\n>>>>>>> incoming\nbetween\n<<<<<<< current\ncurrent two\n||||||| base\nbase two\n=======\nincoming two\n>>>>>>> incoming\nafter";
        let segments = parse_conflict_segments_for_test(
            7,
            "file.txt",
            merged,
            &labels,
            [
                "base one\nbase two\n",
                "current one\ncurrent two\n",
                "incoming one\nincoming two\n",
            ],
        )
        .unwrap();
        let ids: Vec<String> = segments
            .iter()
            .filter_map(|segment| match segment {
                ConflictSegment::Conflict { id, .. } => Some(id.clone()),
                ConflictSegment::Context { .. } => None,
            })
            .collect();
        assert_eq!(ids.len(), 2);
        let rendered = render_conflict_resolution_for_test(
            &segments,
            &[
                ConflictResolution {
                    block_id: ids[0].clone(),
                    choice: ConflictChoice::Both,
                },
                ConflictResolution {
                    block_id: ids[1].clone(),
                    choice: ConflictChoice::Incoming,
                },
            ],
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(rendered).unwrap(),
            "before\ncurrent one\nincoming one\nbetween\nincoming two\nafter"
        );
        assert!(render_conflict_resolution_for_test(&segments, &[]).is_err());
    }

    #[test]
    fn rejects_unsupported_or_incomplete_conflicts() {
        assert!(validate_conflict_text_for_test(b"text\0binary").is_err());
        assert!(validate_conflict_text_for_test(&[0xff]).is_err());
        assert!(validate_conflict_text_for_test(&vec![b'a'; MAX_DIFF_BYTES + 1]).is_err());

        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repo(&git, dir.path());
        git_ok(&git, dir.path(), &["commit", "--allow-empty", "-m", "base"]);
        git_ok(&git, dir.path(), &["switch", "-c", "incoming"]);
        commit_file(&git, dir.path(), "file.txt", "incoming\n", "incoming");
        git_ok(&git, dir.path(), &["switch", "main"]);
        commit_file(&git, dir.path(), "file.txt", "current\n", "current");
        assert!(!git
            .run(dir.path(), &strings(&["merge", "incoming"]), None)
            .unwrap()
            .status
            .success());
        assert!(
            read_conflict_document_for_test(&git, dir.path(), "file.txt", 1)
                .unwrap_err()
                .contains("does not have base")
        );
    }

    #[test]
    fn preserves_missing_eof_newlines_from_git_merge_file() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repo(&git, dir.path());
        commit_file(&git, dir.path(), "file.txt", "base", "base");
        git_ok(&git, dir.path(), &["switch", "-c", "incoming"]);
        commit_file(&git, dir.path(), "file.txt", "incoming", "incoming");
        git_ok(&git, dir.path(), &["switch", "main"]);
        commit_file(&git, dir.path(), "file.txt", "current", "current");
        assert!(!git
            .run(dir.path(), &strings(&["merge", "incoming"]), None)
            .unwrap()
            .status
            .success());
        let document = read_conflict_document_for_test(&git, dir.path(), "file.txt", 1).unwrap();
        let block_id = document
            .segments
            .iter()
            .find_map(|segment| match segment {
                ConflictSegment::Conflict { id, .. } => Some(id.clone()),
                ConflictSegment::Context { .. } => None,
            })
            .unwrap();
        assert_eq!(
            render_conflict_resolution_for_test(
                &document.segments,
                &[ConflictResolution {
                    block_id,
                    choice: ConflictChoice::Current,
                }],
            )
            .unwrap(),
            b"current"
        );
    }

    #[test]
    #[ignore = "performance benchmark: creates 100,000 ignored files"]
    fn benchmarks_status_with_a_large_ignored_tree() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repo(&git, dir.path());
        fs::write(dir.path().join(".gitignore"), "dependencies/\n").unwrap();
        git_ok(&git, dir.path(), &["add", ".gitignore"]);
        git_ok(&git, dir.path(), &["commit", "-m", "ignore dependencies"]);
        for directory in 0..100 {
            let path = dir.path().join("dependencies").join(directory.to_string());
            fs::create_dir_all(&path).unwrap();
            for file in 0..1_000 {
                fs::write(path.join(file.to_string()), b"x").unwrap();
            }
        }
        let mut samples = (0..10)
            .map(|_| {
                let started = std::time::Instant::now();
                let status = read_snapshot(&git, 1, dir.path(), false, 0).unwrap();
                assert!(status.files.is_empty());
                started.elapsed()
            })
            .collect::<Vec<_>>();
        samples.sort();
        println!("ignored-tree-100k status-p95-ms={}", samples[9].as_millis());
        assert!(samples[9] < std::time::Duration::from_secs(1));
    }
}
