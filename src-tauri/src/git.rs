use crate::models::*;
use std::{
    collections::{hash_map::DefaultHasher, HashMap, HashSet},
    ffi::OsStr,
    fs,
    hash::{Hash, Hasher},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

pub const MAX_DIFF_BYTES: usize = 1024 * 1024;
pub const MAX_DIFF_LINES: usize = 20_000;

#[derive(Debug, Clone)]
pub struct Git {
    pub path: PathBuf,
    pub version: String,
}

#[derive(Debug, Clone)]
pub struct RepositoryInspection {
    pub root: PathBuf,
    pub common_git_dir: PathBuf,
    pub git_dir: PathBuf,
    pub bare: bool,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ConflictStage {
    pub mode: String,
    pub oid: String,
}

#[derive(Debug, Clone)]
pub struct ConflictSource {
    pub document: ConflictDocument,
    pub stages: [ConflictStage; 3],
    pub worktree: Vec<u8>,
    pub worktree_executable: bool,
}

impl Git {
    pub fn discover(custom: Option<&str>) -> Result<Self, String> {
        let mut candidates = Vec::new();
        if let Some(path) = custom {
            candidates.push(PathBuf::from(path));
        }
        if let Some(path) = find_on_path("git") {
            candidates.push(path);
        }
        candidates.extend([
            PathBuf::from("/opt/homebrew/bin/git"),
            PathBuf::from("/usr/bin/git"),
        ]);
        candidates.dedup();

        for path in candidates {
            if !path.is_file() {
                continue;
            }
            let output = Command::new(&path).arg("--version").output();
            let Ok(output) = output else { continue };
            if !output.status.success() {
                continue;
            }
            let version = String::from_utf8_lossy(&output.stdout)
                .trim()
                .trim_start_matches("git version ")
                .to_string();
            if version_supported(&version) {
                return Ok(Self { path, version });
            }
        }
        Err(
            "Git 2.30 or newer was not found. Install Git or select its executable in Settings."
                .into(),
        )
    }

    pub fn info(result: &Result<Self, String>) -> GitInfo {
        match result {
            Ok(git) => GitInfo {
                path: Some(git.path.to_string_lossy().into()),
                version: Some(git.version.clone()),
                supported: true,
                error: None,
            },
            Err(error) => GitInfo {
                path: None,
                version: None,
                supported: false,
                error: Some(error.clone()),
            },
        }
    }

    pub fn run(&self, cwd: &Path, args: &[String], input: Option<&[u8]>) -> Result<Output, String> {
        let mut command = Command::new(&self.path);
        command
            .current_dir(cwd)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut child = command
            .spawn()
            .map_err(|e| format!("Cannot start Git: {e}"))?;
        if let Some(bytes) = input {
            child
                .stdin
                .take()
                .ok_or("Cannot open Git stdin")?
                .write_all(bytes)
                .map_err(|e| e.to_string())?;
        }
        child.wait_with_output().map_err(|e| e.to_string())
    }

    pub fn text(&self, cwd: &Path, args: &[&str]) -> Result<String, String> {
        let args = strings(args);
        let output = self.run(cwd, &args, None)?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(error_text(&output))
        }
    }

    pub fn inspect_repository(&self, selected: &Path) -> Result<RepositoryInspection, String> {
        let selected = selected
            .canonicalize()
            .map_err(|e| format!("Cannot access repository path: {e}"))?;
        let bare = self.text(&selected, &["rev-parse", "--is-bare-repository"])? == "true";
        let root = if bare {
            selected.clone()
        } else {
            PathBuf::from(self.text(&selected, &["rev-parse", "--show-toplevel"])?)
        };
        let git_dir = resolve_git_path(&root, &self.text(&root, &["rev-parse", "--git-dir"])?);
        let common_git_dir = resolve_git_path(
            &root,
            &self.text(&root, &["rev-parse", "--git-common-dir"])?,
        );
        Ok(RepositoryInspection {
            root: root.canonicalize().unwrap_or(root),
            common_git_dir,
            git_dir,
            bare,
        })
    }

    pub fn summary(&self, record: &RepositoryRecord) -> RepositorySummary {
        self.summary_with_snapshot(record, 0).0
    }

    pub fn summary_with_snapshot(
        &self,
        record: &RepositoryRecord,
        snapshot_id: u64,
    ) -> (RepositorySummary, Option<WorkingTreeSnapshot>) {
        let missing = || RepositorySummary {
            id: record.id,
            path: record.path.clone(),
            name: record.name.clone(),
            group: record.group.clone(),
            favorite: record.favorite,
            order: record.order,
            kind: RepositoryKind::Missing,
            capabilities: capabilities(RepositoryKind::Missing),
            branch: None,
            head_oid: None,
            changed_count: 0,
            conflict_count: 0,
            ahead: 0,
            behind: 0,
            last_commit: None,
            ongoing: None,
            error: Some("Repository path is unavailable. Relocate or remove this entry.".into()),
        };
        let path = Path::new(&record.path);
        if !path.exists() {
            return (missing(), None);
        }
        let Ok(inspection) = self.inspect_repository(path) else {
            return (missing(), None);
        };
        let kind = if inspection.bare {
            RepositoryKind::Bare
        } else {
            RepositoryKind::WorkTree
        };
        let snapshot = (!inspection.bare)
            .then(|| {
                self.status(record.id, &inspection.root, false, snapshot_id)
                    .ok()
            })
            .flatten();
        let branch = self
            .text(&inspection.root, &["branch", "--show-current"])
            .ok()
            .filter(|s| !s.is_empty());
        let head_oid = snapshot
            .as_ref()
            .and_then(|value| value.head_oid.clone())
            .or_else(|| {
                self.text(&inspection.root, &["rev-parse", "--verify", "HEAD"])
                    .ok()
            });
        let (ahead, behind) = self
            .text(
                &inspection.root,
                &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
            )
            .ok()
            .and_then(|s| {
                let mut n = s.split_whitespace();
                Some((n.next()?.parse().ok()?, n.next()?.parse().ok()?))
            })
            .unwrap_or((0, 0));
        let (changed_count, conflict_count) = snapshot
            .as_ref()
            .map(|value| {
                (
                    value.files.len(),
                    value.files.iter().filter(|file| file.conflict).count(),
                )
            })
            .unwrap_or((0, 0));
        let summary = RepositorySummary {
            id: record.id,
            path: record.path.clone(),
            name: record.name.clone(),
            group: record.group.clone(),
            favorite: record.favorite,
            order: record.order,
            kind: kind.clone(),
            capabilities: capabilities(kind),
            branch,
            head_oid,
            changed_count,
            conflict_count,
            ahead,
            behind,
            last_commit: self
                .text(&inspection.root, &["log", "-1", "--format=%s"])
                .ok(),
            ongoing: ongoing_state(&inspection.git_dir),
            error: None,
        };
        (summary, snapshot)
    }

    pub fn status(
        &self,
        repository_id: RepositoryId,
        cwd: &Path,
        ignored: bool,
        snapshot_id: u64,
    ) -> Result<WorkingTreeSnapshot, String> {
        let mut args = strings(&[
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
        ]);
        if ignored {
            args.push("--ignored=matching".into());
        }
        let output = self.run(cwd, &args, None)?;
        if !output.status.success() {
            return Err(error_text(&output));
        }
        Ok(WorkingTreeSnapshot {
            id: snapshot_id,
            repository_id,
            head_oid: self.text(cwd, &["rev-parse", "--verify", "HEAD"]).ok(),
            files: parse_porcelain_v2(&output.stdout),
        })
    }

    pub fn diff(
        &self,
        cwd: &Path,
        path: &str,
        staged: bool,
        snapshot_id: u64,
    ) -> Result<DiffFile, String> {
        let mut args = strings(&["diff", "--no-ext-diff", "--no-color", "--unified=3"]);
        if staged {
            args.push("--cached".into());
        }
        args.extend(["--".into(), path.into()]);
        let output = self.run(cwd, &args, None)?;
        if !output.status.success() {
            return Err(error_text(&output));
        }
        let patch = String::from_utf8_lossy(&output.stdout).to_string();
        let binary = patch.contains("GIT binary patch") || patch.contains("Binary files ");
        let too_large = patch.len() > MAX_DIFF_BYTES || patch.lines().count() > MAX_DIFF_LINES;
        let (shown, hunks) = if binary || too_large {
            (String::new(), Vec::new())
        } else {
            let hunks = split_hunks(snapshot_id, path, staged, &patch);
            (patch, hunks)
        };
        Ok(DiffFile {
            path: path.into(),
            staged,
            binary,
            too_large,
            patch: shown,
            hunks,
        })
    }

    pub fn conflict_source(
        &self,
        cwd: &Path,
        path: &str,
        snapshot_id: u64,
    ) -> Result<ConflictSource, String> {
        let stages = self.conflict_stages(cwd, path)?;
        let contents = stages
            .iter()
            .map(|stage| self.filtered_blob(cwd, path, &stage.oid))
            .collect::<Result<Vec<_>, _>>()?;
        let [base, current, incoming]: [Vec<u8>; 3] = contents
            .try_into()
            .map_err(|_| "A three-stage conflict is required".to_string())?;
        for content in [&base, &current, &incoming] {
            validate_conflict_text(content)?;
        }
        let worktree = fs::read(cwd.join(path)).map_err(|error| error.to_string())?;
        validate_conflict_text(&worktree)?;
        let worktree_executable = file_executable(&cwd.join(path))?;

        let mut hasher = DefaultHasher::new();
        (snapshot_id, path, &stages).hash(&mut hasher);
        let tag = format!("{:016x}", hasher.finish());
        let labels = [
            format!("gitdock-current-{tag}"),
            format!("gitdock-base-{tag}"),
            format!("gitdock-incoming-{tag}"),
        ];
        let mut current_file = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
        let mut base_file = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
        let mut incoming_file =
            tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
        current_file
            .write_all(&current)
            .map_err(|error| error.to_string())?;
        base_file
            .write_all(&base)
            .map_err(|error| error.to_string())?;
        incoming_file
            .write_all(&incoming)
            .map_err(|error| error.to_string())?;
        let output = self.run(
            cwd,
            &[
                "merge-file".into(),
                "--diff3".into(),
                "--stdout".into(),
                "-L".into(),
                labels[0].clone(),
                "-L".into(),
                labels[1].clone(),
                "-L".into(),
                labels[2].clone(),
                current_file.path().to_string_lossy().into_owned(),
                base_file.path().to_string_lossy().into_owned(),
                incoming_file.path().to_string_lossy().into_owned(),
            ],
            None,
        )?;
        if !matches!(output.status.code(), Some(0..=127)) {
            return Err(error_text(&output));
        }
        let merged = String::from_utf8(output.stdout)
            .map_err(|_| "Conflict content is not valid UTF-8".to_string())?;
        let originals = [
            std::str::from_utf8(&base).unwrap(),
            std::str::from_utf8(&current).unwrap(),
            std::str::from_utf8(&incoming).unwrap(),
        ];
        let segments = parse_conflict_segments(snapshot_id, path, &merged, &labels, originals)?;
        let mut document_hasher = DefaultHasher::new();
        (
            snapshot_id,
            path,
            &stages,
            &merged,
            &worktree,
            worktree_executable,
        )
            .hash(&mut document_hasher);
        Ok(ConflictSource {
            document: ConflictDocument {
                id: format!("{:016x}", document_hasher.finish()),
                path: path.into(),
                segments,
            },
            stages,
            worktree,
            worktree_executable,
        })
    }

    pub fn conflict_stages(&self, cwd: &Path, path: &str) -> Result<[ConflictStage; 3], String> {
        let output = ensure_success(self.run(
            cwd,
            &strings(&["ls-files", "--unmerged", "-z", "--", path]),
            None,
        )?)?;
        let mut stages: [Option<ConflictStage>; 3] = [None, None, None];
        for record in output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
        {
            let record = std::str::from_utf8(record)
                .map_err(|_| "Conflict index entry is not valid UTF-8")?;
            let (metadata, entry_path) = record
                .split_once('\t')
                .ok_or("Invalid conflict index entry")?;
            if entry_path != path {
                return Err("Conflict index path changed".into());
            }
            let mut fields = metadata.split_whitespace();
            let mode = fields.next().ok_or("Conflict stage mode is missing")?;
            let oid = fields.next().ok_or("Conflict stage object is missing")?;
            let stage = fields
                .next()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|stage| (1..=3).contains(stage))
                .ok_or("Invalid conflict stage")?;
            if stages[stage - 1].is_some() {
                return Err("Duplicate conflict stage".into());
            }
            stages[stage - 1] = Some(ConflictStage {
                mode: mode.into(),
                oid: oid.into(),
            });
        }
        let [Some(base), Some(current), Some(incoming)] = stages else {
            return Err("This conflict does not have base, current, and incoming stages".into());
        };
        if !matches!(base.mode.as_str(), "100644" | "100755")
            || current.mode != base.mode
            || incoming.mode != base.mode
        {
            return Err(
                "Only regular files with matching modes can use the internal conflict editor"
                    .into(),
            );
        }
        Ok([base, current, incoming])
    }

    fn filtered_blob(&self, cwd: &Path, path: &str, oid: &str) -> Result<Vec<u8>, String> {
        let output = ensure_success(self.run(
            cwd,
            &[
                "cat-file".into(),
                "--filters".into(),
                format!("--path={path}"),
                oid.into(),
            ],
            None,
        )?)?;
        Ok(output.stdout)
    }

    pub fn history(
        &self,
        cwd: &Path,
        cursor: Option<HistoryCursor>,
        limit: usize,
    ) -> Result<CommitPage, String> {
        let HistoryCursor {
            offset,
            active_lanes,
        } = cursor.unwrap_or(HistoryCursor {
            offset: 0,
            active_lanes: Vec::new(),
        });
        let format = "%H%x1f%P%x1f%an%x1f%aI%x1f%D%x1f%s%x1e";
        let args = vec![
            "log".into(),
            "--all".into(),
            "--topo-order".into(),
            format!("--skip={offset}"),
            format!("--max-count={}", limit + 1),
            format!("--format={format}"),
        ];
        let output = self.run(cwd, &args, None)?;
        if !output.status.success() {
            return Err(error_text(&output));
        }
        let mut commits = parse_history(&String::from_utf8_lossy(&output.stdout));
        let has_more = commits.len() > limit;
        commits.truncate(limit);
        let active_lanes = assign_lanes(&mut commits, active_lanes);
        Ok(CommitPage {
            commits,
            next_cursor: has_more.then_some(HistoryCursor {
                offset: offset.saturating_add(limit),
                active_lanes,
            }),
        })
    }

    pub fn branches(&self, cwd: &Path) -> Result<Vec<BranchInfo>, String> {
        let text = self.text(
            cwd,
            &[
                "for-each-ref",
                "--format=%(refname)%09%(objectname)%09%(HEAD)%09%(upstream:short)",
                "refs/heads",
                "refs/remotes",
            ],
        )?;
        Ok(text
            .lines()
            .filter_map(|line| {
                let mut p = line.split('\t');
                let full = p.next()?;
                let oid = p.next()?.into();
                let current = p.next()? == "*";
                let upstream = p.next().filter(|s| !s.is_empty()).map(Into::into);
                let remote = full.starts_with("refs/remotes/");
                Some(BranchInfo {
                    name: full
                        .trim_start_matches(if remote {
                            "refs/remotes/"
                        } else {
                            "refs/heads/"
                        })
                        .into(),
                    oid,
                    current,
                    remote,
                    upstream,
                })
            })
            .collect())
    }

    pub fn tags(&self, cwd: &Path) -> Result<Vec<TagInfo>, String> {
        let text = self.text(
            cwd,
            &[
                "for-each-ref",
                "--format=%(refname:short)%09%(objectname)%09%(subject)",
                "refs/tags",
            ],
        )?;
        Ok(text
            .lines()
            .filter_map(|line| {
                let mut p = line.splitn(3, '\t');
                Some(TagInfo {
                    name: p.next()?.into(),
                    oid: p.next()?.into(),
                    subject: p.next().unwrap_or_default().into(),
                })
            })
            .collect())
    }

    pub fn remotes(&self, cwd: &Path) -> Result<Vec<RemoteInfo>, String> {
        let names = self.text(cwd, &["remote"])?;
        Ok(names
            .lines()
            .map(|name| RemoteInfo {
                name: name.into(),
                fetch_url: redact_url(
                    &self
                        .text(cwd, &["remote", "get-url", name])
                        .unwrap_or_default(),
                ),
                push_url: redact_url(
                    &self
                        .text(cwd, &["remote", "get-url", "--push", name])
                        .unwrap_or_default(),
                ),
            })
            .collect())
    }

    pub fn stashes(&self, cwd: &Path) -> Result<Vec<StashInfo>, String> {
        let text = self.text(cwd, &["stash", "list", "--format=%gd%x09%H%x09%s"])?;
        Ok(text
            .lines()
            .filter_map(|line| {
                let mut p = line.splitn(3, '\t');
                let index = p
                    .next()?
                    .trim_start_matches("stash@{")
                    .trim_end_matches('}')
                    .parse()
                    .ok()?;
                Some(StashInfo {
                    index,
                    oid: p.next()?.into(),
                    subject: p.next().unwrap_or_default().into(),
                })
            })
            .collect())
    }

    pub fn submodules(&self, cwd: &Path) -> Result<Vec<SubmoduleInfo>, String> {
        let output = self.run(cwd, &strings(&["submodule", "status"]), None)?;
        if !output.status.success() && !output.stderr.is_empty() {
            return Err(error_text(&output));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let state_char = line.chars().next()?;
                let mut p = line[1..].split_whitespace();
                Some(SubmoduleInfo {
                    oid: p.next()?.into(),
                    path: p.next()?.into(),
                    initialized: state_char != '-',
                    state: match state_char {
                        '+' => "changed",
                        '-' => "uninitialized",
                        'U' => "conflicted",
                        _ => "clean",
                    }
                    .into(),
                })
            })
            .collect())
    }
}

pub fn render_conflict_resolution(
    segments: &[ConflictSegment],
    choices: &[ConflictResolution],
) -> Result<Vec<u8>, String> {
    let mut selected = HashMap::new();
    for choice in choices {
        if selected
            .insert(choice.block_id.as_str(), &choice.choice)
            .is_some()
        {
            return Err("A conflict block was selected more than once".into());
        }
    }
    let expected: HashSet<&str> = segments
        .iter()
        .filter_map(|segment| match segment {
            ConflictSegment::Conflict { id, .. } => Some(id.as_str()),
            ConflictSegment::Context { .. } => None,
        })
        .collect();
    if selected.len() != expected.len() || selected.keys().any(|id| !expected.contains(id)) {
        return Err("Choose a resolution for every conflict block".into());
    }
    let mut result = String::new();
    for segment in segments {
        match segment {
            ConflictSegment::Context { text } => result.push_str(text),
            ConflictSegment::Conflict {
                id,
                current,
                incoming,
                ..
            } => match selected[id.as_str()] {
                ConflictChoice::Current => result.push_str(current),
                ConflictChoice::Incoming => result.push_str(incoming),
                ConflictChoice::Both => {
                    result.push_str(current);
                    result.push_str(incoming);
                }
            },
        }
    }
    Ok(result.into_bytes())
}

pub fn file_executable(path: &Path) -> Result<bool, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .map_err(|error| error.to_string());
    }
    #[cfg(not(unix))]
    {
        fs::metadata(path)
            .map(|_| false)
            .map_err(|error| error.to_string())
    }
}

fn validate_conflict_text(content: &[u8]) -> Result<(), String> {
    if content.len() > MAX_DIFF_BYTES
        || content.iter().filter(|byte| **byte == b'\n').count() + 1 > MAX_DIFF_LINES
    {
        return Err("This conflict is too large for the internal editor".into());
    }
    if content.contains(&0) {
        return Err("Binary conflicts must use an external merge tool".into());
    }
    std::str::from_utf8(content)
        .map(|_| ())
        .map_err(|_| "Conflict content is not valid UTF-8".into())
}

fn parse_conflict_segments(
    snapshot_id: u64,
    path: &str,
    merged: &str,
    labels: &[String; 3],
    originals: [&str; 3],
) -> Result<Vec<ConflictSegment>, String> {
    let lines: Vec<&str> = merged.split_inclusive('\n').collect();
    let marker = |line: &str, prefix: &str, label: &str| {
        line.trim_end_matches(['\r', '\n']) == format!("{prefix} {label}")
    };
    let mut segments = Vec::new();
    let mut context = String::new();
    let mut index = 0;
    let mut block_index = 0;
    while index < lines.len() {
        if !marker(lines[index], "<<<<<<<", &labels[0]) {
            context.push_str(lines[index]);
            index += 1;
            continue;
        }
        if !context.is_empty() {
            segments.push(ConflictSegment::Context {
                text: std::mem::take(&mut context),
            });
        }
        index += 1;
        let mut current = String::new();
        while index < lines.len() && !marker(lines[index], "|||||||", &labels[1]) {
            current.push_str(lines[index]);
            index += 1;
        }
        if index == lines.len() {
            return Err("Git returned an incomplete conflict block".into());
        }
        index += 1;
        let mut base = String::new();
        while index < lines.len() && lines[index].trim_end_matches(['\r', '\n']) != "=======" {
            base.push_str(lines[index]);
            index += 1;
        }
        if index == lines.len() {
            return Err("Git returned an incomplete conflict block".into());
        }
        index += 1;
        let mut incoming = String::new();
        while index < lines.len() && !marker(lines[index], ">>>>>>>", &labels[2]) {
            incoming.push_str(lines[index]);
            index += 1;
        }
        if index == lines.len() {
            return Err("Git returned an incomplete conflict block".into());
        }
        index += 1;
        segments.push(ConflictSegment::Conflict {
            id: String::new(),
            base,
            current,
            incoming,
        });
        block_index += 1;
    }
    if !context.is_empty() {
        segments.push(ConflictSegment::Context { text: context });
    }
    if block_index == 0 {
        return Err("No text conflict blocks were found".into());
    }
    if let Some(ConflictSegment::Conflict {
        base,
        current,
        incoming,
        ..
    }) = segments
        .iter_mut()
        .rev()
        .find(|segment| matches!(segment, ConflictSegment::Conflict { .. }))
    {
        restore_missing_eof_newline(base, originals[0]);
        restore_missing_eof_newline(current, originals[1]);
        restore_missing_eof_newline(incoming, originals[2]);
    }
    let mut block_index = 0;
    for segment in &mut segments {
        if let ConflictSegment::Conflict {
            id,
            base,
            current,
            incoming,
        } = segment
        {
            let mut hasher = DefaultHasher::new();
            (snapshot_id, path, block_index, base, current, incoming).hash(&mut hasher);
            *id = format!("{:016x}", hasher.finish());
            block_index += 1;
        }
    }
    Ok(segments)
}

fn restore_missing_eof_newline(chunk: &mut String, original: &str) {
    if !original.ends_with('\n') && chunk.ends_with('\n') {
        let without_newline = &chunk[..chunk.len() - 1];
        if original.ends_with(without_newline) {
            chunk.pop();
        }
    }
}

pub fn parse_porcelain_v2(bytes: &[u8]) -> Vec<FileChange> {
    let records: Vec<&[u8]> = bytes.split(|b| *b == 0).filter(|r| !r.is_empty()).collect();
    let mut files = Vec::new();
    let mut i = 0;
    while i < records.len() {
        let record = String::from_utf8_lossy(records[i]);
        let first = record.as_bytes().first().copied();
        match first {
            Some(b'1') => {
                if let Some(file) = parse_ordinary(&record, 8, None) {
                    files.push(file);
                }
            }
            Some(b'2') => {
                let original = records
                    .get(i + 1)
                    .map(|r| String::from_utf8_lossy(r).into_owned());
                if let Some(file) = parse_ordinary(&record, 9, original) {
                    files.push(file);
                }
                i += 1;
            }
            Some(b'u') => {
                let fields: Vec<&str> = record.splitn(11, ' ').collect();
                if fields.len() == 11 {
                    files.push(change(fields[10], None, "U", "U", false, true));
                }
            }
            Some(b'?') => files.push(change(
                record.get(2..).unwrap_or_default(),
                None,
                "?",
                "?",
                false,
                false,
            )),
            Some(b'!') => files.push(change(
                record.get(2..).unwrap_or_default(),
                None,
                "!",
                "!",
                true,
                false,
            )),
            _ => {}
        }
        i += 1;
    }
    files
}

fn parse_ordinary(record: &str, path_index: usize, original: Option<String>) -> Option<FileChange> {
    let fields: Vec<&str> = record.splitn(path_index + 1, ' ').collect();
    let xy = *fields.get(1)?;
    let path = *fields.get(path_index)?;
    let mut chars = xy.chars();
    Some(change(
        path,
        original,
        &chars.next()?.to_string(),
        &chars.next()?.to_string(),
        false,
        false,
    ))
}

fn change(
    path: &str,
    original_path: Option<String>,
    index: &str,
    worktree: &str,
    ignored: bool,
    forced_conflict: bool,
) -> FileChange {
    let conflict = forced_conflict
        || matches!(
            (index, worktree),
            ("D", "D")
                | ("A", "U")
                | ("U", "D")
                | ("U", "A")
                | ("D", "U")
                | ("A", "A")
                | ("U", "U")
        );
    let code = if conflict {
        "U"
    } else if index != "." && index != "?" && index != "!" {
        index
    } else {
        worktree
    };
    let kind = match code {
        "A" => ChangeKind::Added,
        "M" => ChangeKind::Modified,
        "D" => ChangeKind::Deleted,
        "R" => ChangeKind::Renamed,
        "C" => ChangeKind::Copied,
        "T" => ChangeKind::TypeChanged,
        "?" => ChangeKind::Untracked,
        "!" => ChangeKind::Ignored,
        "U" => ChangeKind::Conflicted,
        _ => ChangeKind::Unknown,
    };
    FileChange {
        path: path.into(),
        original_path,
        kind,
        index_status: Some(index.into()),
        worktree_status: Some(worktree.into()),
        staged: !matches!(index, "." | "?" | "!"),
        unstaged: !matches!(worktree, "." | "!"),
        conflict,
        ignored,
    }
}

fn split_hunks(snapshot_id: u64, path: &str, staged: bool, patch: &str) -> Vec<DiffHunk> {
    let Some(first) = patch.find("@@") else {
        return Vec::new();
    };
    let header = &patch[..first];
    let mut starts: Vec<usize> = patch
        .match_indices("@@")
        .filter(|(i, _)| *i == first || patch[..*i].ends_with('\n'))
        .map(|(i, _)| i)
        .collect();
    starts.push(patch.len());
    starts
        .windows(2)
        .enumerate()
        .map(|(index, range)| {
            let body = &patch[range[0]..range[1]];
            let hunk_header = body.lines().next().unwrap_or("@@").to_string();
            let complete = format!("{header}{body}");
            let mut hasher = DefaultHasher::new();
            (snapshot_id, path, staged, index, &complete).hash(&mut hasher);
            DiffHunk {
                id: format!("{:016x}", hasher.finish()),
                header: hunk_header,
                patch: complete,
            }
        })
        .collect()
}

fn parse_history(text: &str) -> Vec<CommitInfo> {
    text.split('\x1e')
        .filter_map(|record| {
            let record = record.trim_matches('\n');
            if record.is_empty() {
                return None;
            }
            let mut p = record.splitn(6, '\x1f');
            Some(CommitInfo {
                oid: p.next()?.into(),
                parents: p.next()?.split_whitespace().map(Into::into).collect(),
                author: p.next()?.into(),
                authored_at: p.next()?.into(),
                refs: p
                    .next()?
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(Into::into)
                    .collect(),
                subject: p.next().unwrap_or_default().into(),
                lane: GraphLane {
                    column: 0,
                    parent_columns: Vec::new(),
                },
            })
        })
        .collect()
}

fn assign_lanes(commits: &mut [CommitInfo], mut active: Vec<String>) -> Vec<String> {
    for commit in commits {
        let column = active
            .iter()
            .position(|oid| oid == &commit.oid)
            .unwrap_or_else(|| {
                active.insert(0, commit.oid.clone());
                0
            });
        active.remove(column);
        for (offset, parent) in commit.parents.iter().enumerate() {
            if active.contains(parent) {
                continue;
            }
            active.insert((column + offset).min(active.len()), parent.clone());
        }
        commit.lane = GraphLane {
            column,
            parent_columns: commit
                .parents
                .iter()
                .filter_map(|p| active.iter().position(|oid| oid == p))
                .collect(),
        };
    }
    active
}

pub fn ongoing_state(git_dir: &Path) -> Option<OngoingGitState> {
    let state = if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
        (OngoingKind::Rebase, true, true, true)
    } else if git_dir.join("MERGE_HEAD").exists() {
        (OngoingKind::Merge, true, false, true)
    } else if git_dir.join("CHERRY_PICK_HEAD").exists() {
        (OngoingKind::CherryPick, true, true, true)
    } else if git_dir.join("REVERT_HEAD").exists() {
        (OngoingKind::Revert, true, true, true)
    } else {
        return None;
    };
    Some(OngoingGitState {
        kind: state.0,
        can_continue: state.1,
        can_skip: state.2,
        can_abort: state.3,
    })
}

fn capabilities(kind: RepositoryKind) -> RepositoryCapabilities {
    match kind {
        RepositoryKind::WorkTree => RepositoryCapabilities {
            can_read: true,
            can_write_work_tree: true,
            can_manage_refs: true,
            can_manage_remotes: true,
        },
        RepositoryKind::Bare => RepositoryCapabilities {
            can_read: true,
            can_write_work_tree: false,
            can_manage_refs: false,
            can_manage_remotes: false,
        },
        RepositoryKind::Missing => RepositoryCapabilities {
            can_read: false,
            can_write_work_tree: false,
            can_manage_refs: false,
            can_manage_remotes: false,
        },
    }
}

pub fn error_text(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        redact_url(&stderr)
    }
}

pub fn redact_url(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(scheme) = remaining.find("://") {
        let credentials = scheme + 3;
        output.push_str(&remaining[..credentials]);
        let authority_end = remaining[credentials..]
            .find(|character: char| character == '/' || character.is_whitespace())
            .map(|offset| credentials + offset)
            .unwrap_or(remaining.len());
        let authority = &remaining[credentials..authority_end];
        if let Some(at) = authority.rfind('@') {
            output.push_str("***@");
            output.push_str(&authority[at + 1..]);
        } else {
            output.push_str(authority);
        }
        remaining = &remaining[authority_end..];
    }
    output.push_str(remaining);
    output
}

fn resolve_git_path(root: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    let path = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    path.canonicalize().unwrap_or(path)
}

fn version_supported(version: &str) -> bool {
    let mut parts = version.split('.').filter_map(|p| {
        p.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse::<u32>()
            .ok()
    });
    matches!((parts.next(), parts.next()), (Some(major), Some(minor)) if major > 2 || major == 2 && minor >= 30)
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_string_lossy()
        .split(':')
        .map(Path::new)
        .map(|p| p.join(name))
        .find(|p| p.is_file())
}

pub fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).into()).collect()
}

pub fn ensure_success(output: Output) -> Result<Output, String> {
    if output.status.success() {
        Ok(output)
    } else {
        Err(error_text(&output))
    }
}

pub fn path_name(path: &Path) -> String {
    path.file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("Repository")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repository(git: &Git, path: &Path) {
        ensure_success(
            git.run(path, &strings(&["init", "-b", "main"]), None)
                .unwrap(),
        )
        .unwrap();
        for (key, value) in [
            ("user.name", "GitDock Test"),
            ("user.email", "test@gitdock.local"),
        ] {
            ensure_success(
                git.run(path, &strings(&["config", key, value]), None)
                    .unwrap(),
            )
            .unwrap();
        }
    }

    fn commit_file(git: &Git, path: &Path, contents: &str, message: &str) -> String {
        std::fs::write(path.join("file.txt"), contents).unwrap();
        ensure_success(
            git.run(path, &strings(&["add", "--", "file.txt"]), None)
                .unwrap(),
        )
        .unwrap();
        ensure_success(
            git.run(path, &strings(&["commit", "-m", message]), None)
                .unwrap(),
        )
        .unwrap();
        git.text(path, &["rev-parse", "HEAD"]).unwrap()
    }

    #[test]
    fn parses_and_resolves_multiple_conflict_blocks() {
        let labels = ["current".into(), "base".into(), "incoming".into()];
        let merged = "before\n<<<<<<< current\ncurrent one\n||||||| base\nbase one\n=======\nincoming one\n>>>>>>> incoming\nbetween\n<<<<<<< current\ncurrent two\n||||||| base\nbase two\n=======\nincoming two\n>>>>>>> incoming\nafter";
        let segments = parse_conflict_segments(
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
        assert_ne!(ids[0], ids[1]);
        let rendered = render_conflict_resolution(
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
        assert!(render_conflict_resolution(&segments, &[]).is_err());
        assert!(render_conflict_resolution(
            &segments,
            &[
                ConflictResolution {
                    block_id: ids[0].clone(),
                    choice: ConflictChoice::Current,
                },
                ConflictResolution {
                    block_id: ids[0].clone(),
                    choice: ConflictChoice::Incoming,
                },
            ],
        )
        .is_err());
    }

    #[test]
    fn rejects_unsupported_conflict_content() {
        assert!(validate_conflict_text(b"text\0binary").is_err());
        assert!(validate_conflict_text(&[0xff]).is_err());
        assert!(validate_conflict_text(&vec![b'a'; MAX_DIFF_BYTES + 1]).is_err());
    }

    #[test]
    fn rejects_conflicts_without_all_three_stages() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repository(&git, dir.path());
        ensure_success(
            git.run(
                dir.path(),
                &strings(&["commit", "--allow-empty", "-m", "base"]),
                None,
            )
            .unwrap(),
        )
        .unwrap();
        ensure_success(
            git.run(dir.path(), &strings(&["switch", "-c", "incoming"]), None)
                .unwrap(),
        )
        .unwrap();
        commit_file(&git, dir.path(), "incoming\n", "incoming");
        ensure_success(
            git.run(dir.path(), &strings(&["switch", "main"]), None)
                .unwrap(),
        )
        .unwrap();
        commit_file(&git, dir.path(), "current\n", "current");
        assert!(!git
            .run(dir.path(), &strings(&["merge", "incoming"]), None)
            .unwrap()
            .status
            .success());
        assert!(git
            .conflict_source(dir.path(), "file.txt", 1)
            .unwrap_err()
            .contains("does not have base"));
    }

    #[test]
    fn preserves_missing_eof_newlines_from_git_merge_file() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repository(&git, dir.path());
        commit_file(&git, dir.path(), "base", "base");
        ensure_success(
            git.run(dir.path(), &strings(&["switch", "-c", "incoming"]), None)
                .unwrap(),
        )
        .unwrap();
        commit_file(&git, dir.path(), "incoming", "incoming");
        ensure_success(
            git.run(dir.path(), &strings(&["switch", "main"]), None)
                .unwrap(),
        )
        .unwrap();
        commit_file(&git, dir.path(), "current", "current");
        assert!(!git
            .run(dir.path(), &strings(&["merge", "incoming"]), None)
            .unwrap()
            .status
            .success());
        let source = git.conflict_source(dir.path(), "file.txt", 1).unwrap();
        let block_id = source
            .document
            .segments
            .iter()
            .find_map(|segment| match segment {
                ConflictSegment::Conflict { id, .. } => Some(id.clone()),
                ConflictSegment::Context { .. } => None,
            })
            .unwrap();
        assert_eq!(
            render_conflict_resolution(
                &source.document.segments,
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
    fn parses_zero_delimited_status_and_rename() {
        let input = b"1 M. N... 100644 100644 100644 a a src/a.rs\0? new file.txt\02 R. N... 100644 100644 100644 a a R100 new.rs\0old.rs\0! target/a\0";
        let files = parse_porcelain_v2(input);
        assert_eq!(files.len(), 4);
        assert!(files[0].staged);
        assert_eq!(files[1].kind, ChangeKind::Untracked);
        assert_eq!(files[2].original_path.as_deref(), Some("old.rs"));
        assert!(files[3].ignored);
    }

    #[test]
    fn summary_and_snapshot_share_worktree_status() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repository(&git, dir.path());
        let head = commit_file(&git, dir.path(), "base\n", "base");
        std::fs::write(dir.path().join("file.txt"), "changed\n").unwrap();
        let record = RepositoryRecord {
            id: 7,
            path: dir.path().to_string_lossy().into_owned(),
            name: "repo".into(),
            group: None,
            favorite: false,
            order: 0,
        };

        let (summary, snapshot) = git.summary_with_snapshot(&record, 42);
        let snapshot = snapshot.unwrap();
        assert_eq!(snapshot.id, 42);
        assert_eq!(snapshot.head_oid.as_deref(), Some(head.as_str()));
        assert_eq!(summary.head_oid, snapshot.head_oid);
        assert_eq!(summary.changed_count, snapshot.files.len());
        assert_eq!(summary.conflict_count, 0);
        assert_eq!(snapshot.files[0].path, "file.txt");
    }

    #[test]
    fn splits_hunks_into_backend_owned_patches() {
        let patch =
            "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-a\n+b\n@@ -3 +3 @@\n-c\n+d\n";
        let hunks = split_hunks(3, "a", false, patch);
        assert_eq!(hunks.len(), 2);
        assert!(hunks.iter().all(|h| h.patch.starts_with("diff --git")));
        assert_ne!(hunks[0].id, hunks[1].id);
    }

    #[test]
    fn redacts_http_userinfo() {
        assert_eq!(
            redact_url("https://user:token@example.com/repo"),
            "https://***@example.com/repo"
        );
        assert_eq!(
            redact_url("from https://a:b@one.test/x to https://c:d@two.test/y"),
            "from https://***@one.test/x to https://***@two.test/y"
        );
        assert_eq!(redact_url("git@example.com:repo"), "git@example.com:repo");
    }

    #[test]
    fn lanes_continue_across_pages_and_compact_wide_merges() {
        let commit = |oid: &str, parents: &[&str]| CommitInfo {
            oid: oid.into(),
            parents: parents.iter().map(|parent| (*parent).into()).collect(),
            author: String::new(),
            authored_at: String::new(),
            subject: String::new(),
            refs: Vec::new(),
            lane: GraphLane {
                column: 0,
                parent_columns: Vec::new(),
            },
        };
        let mut first = vec![commit("merge", &["a", "b", "c"]), commit("a", &["d"])];
        let active = assign_lanes(&mut first, Vec::new());
        assert_eq!(first[0].lane.parent_columns, [0, 1, 2]);
        assert_eq!(active, ["d", "b", "c"]);

        let mut second = vec![commit("b", &["d"]), commit("c", &["d"]), commit("d", &[])];
        let active = assign_lanes(&mut second, active);
        assert_eq!(
            second
                .iter()
                .map(|item| item.lane.column)
                .collect::<Vec<_>>(),
            [1, 1, 0]
        );
        assert!(active.is_empty());
    }

    #[test]
    #[ignore = "performance benchmark: creates 100,000 commits"]
    fn benchmarks_history_on_one_hundred_thousand_commits() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repository(&git, dir.path());
        let mut stream = String::with_capacity(16_000_000);
        for index in 1..=100_000 {
            let subject = format!("commit {index}");
            stream.push_str(&format!(
                "commit refs/heads/main\nmark :{index}\nauthor GitDock Test <test@gitdock.local> {index} +0000\ncommitter GitDock Test <test@gitdock.local> {index} +0000\ndata {}\n{subject}\n",
                subject.len()
            ));
            if index > 1 {
                stream.push_str(&format!("from :{}\n", index - 1));
            }
            stream.push('\n');
        }
        for branch in 0..8 {
            let mark = 100_001 + branch;
            let subject = format!("wide branch {branch}");
            stream.push_str(&format!(
                "commit refs/heads/wide-{branch}\nmark :{mark}\nauthor GitDock Test <test@gitdock.local> {mark} +0000\ncommitter GitDock Test <test@gitdock.local> {mark} +0000\ndata {}\n{subject}\nfrom :100000\n\n",
                subject.len()
            ));
        }
        let subject = "wide octopus merge";
        stream.push_str(&format!(
            "commit refs/heads/main\nmark :100009\nauthor GitDock Test <test@gitdock.local> 100009 +0000\ncommitter GitDock Test <test@gitdock.local> 100009 +0000\ndata {}\n{subject}\nfrom :100000\n",
            subject.len()
        ));
        for mark in 100_001..=100_008 {
            stream.push_str(&format!("merge :{mark}\n"));
        }
        stream.push('\n');
        ensure_success(
            git.run(
                dir.path(),
                &strings(&["fast-import", "--quiet"]),
                Some(stream.as_bytes()),
            )
            .unwrap(),
        )
        .unwrap();

        let measure = |cursor| {
            let started = std::time::Instant::now();
            let page = git.history(dir.path(), cursor, 100).unwrap();
            assert_eq!(page.commits.len(), 100);
            started.elapsed()
        };
        let mut first = (0..10).map(|_| measure(None)).collect::<Vec<_>>();
        let deep_cursor = HistoryCursor {
            offset: 50_000,
            active_lanes: Vec::new(),
        };
        let mut deep = (0..10)
            .map(|_| measure(Some(deep_cursor.clone())))
            .collect::<Vec<_>>();
        first.sort();
        deep.sort();
        println!(
            "history-100k first-page-p95-ms={} deep-page-p95-ms={}",
            first[9].as_millis(),
            deep[9].as_millis()
        );
        assert!(first[9] < std::time::Duration::from_secs(1));
        assert!(deep[9] < std::time::Duration::from_secs(1));
    }

    #[test]
    #[ignore = "performance benchmark: creates 100,000 ignored files"]
    fn benchmarks_status_with_a_large_ignored_tree() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repository(&git, dir.path());
        std::fs::write(dir.path().join(".gitignore"), "dependencies/\n").unwrap();
        ensure_success(
            git.run(dir.path(), &strings(&["add", ".gitignore"]), None)
                .unwrap(),
        )
        .unwrap();
        ensure_success(
            git.run(
                dir.path(),
                &strings(&["commit", "-m", "ignore dependencies"]),
                None,
            )
            .unwrap(),
        )
        .unwrap();
        for directory in 0..100 {
            let path = dir.path().join("dependencies").join(directory.to_string());
            std::fs::create_dir_all(&path).unwrap();
            for file in 0..1_000 {
                std::fs::write(path.join(file.to_string()), b"x").unwrap();
            }
        }
        let mut samples = (0..10)
            .map(|_| {
                let started = std::time::Instant::now();
                let status = git.status(1, dir.path(), false, 0).unwrap();
                assert!(status.files.is_empty());
                started.elapsed()
            })
            .collect::<Vec<_>>();
        samples.sort();
        println!("ignored-tree-100k status-p95-ms={}", samples[9].as_millis());
        assert!(samples[9] < std::time::Duration::from_secs(1));
    }

    #[test]
    fn reads_a_real_repository_end_to_end() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        ensure_success(git.run(dir.path(), &strings(&["init"]), None).unwrap()).unwrap();
        ensure_success(
            git.run(
                dir.path(),
                &strings(&["config", "user.name", "GitDock Test"]),
                None,
            )
            .unwrap(),
        )
        .unwrap();
        ensure_success(
            git.run(
                dir.path(),
                &strings(&["config", "user.email", "test@gitdock.local"]),
                None,
            )
            .unwrap(),
        )
        .unwrap();
        std::fs::write(dir.path().join("hello world.txt"), "one\ntwo\n").unwrap();

        let untracked = git.status(1, dir.path(), false, 1).unwrap();
        assert_eq!(untracked.files[0].kind, ChangeKind::Untracked);
        ensure_success(
            git.run(
                dir.path(),
                &strings(&["add", "--", "hello world.txt"]),
                None,
            )
            .unwrap(),
        )
        .unwrap();
        let staged = git.status(1, dir.path(), false, 2).unwrap();
        assert!(staged.files[0].staged);
        ensure_success(
            git.run(dir.path(), &strings(&["commit", "-m", "initial"]), None)
                .unwrap(),
        )
        .unwrap();

        std::fs::write(
            dir.path().join("hello world.txt"),
            "one changed\ntwo\nthree\n",
        )
        .unwrap();
        let diff = git.diff(dir.path(), "hello world.txt", false, 3).unwrap();
        assert!(!diff.hunks.is_empty());
        std::fs::write(
            dir.path().join("hello world.txt"),
            "changed after snapshot\n",
        )
        .unwrap();
        assert_ne!(
            git.diff(dir.path(), "hello world.txt", false, 3)
                .unwrap()
                .patch,
            diff.patch
        );
        let history = git.history(dir.path(), None, 20).unwrap();
        assert_eq!(history.commits[0].subject, "initial");
    }

    #[test]
    fn identifies_bare_repositories() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        ensure_success(
            git.run(dir.path(), &strings(&["init", "--bare"]), None)
                .unwrap(),
        )
        .unwrap();
        assert!(git.inspect_repository(dir.path()).unwrap().bare);
    }

    #[cfg(unix)]
    #[test]
    fn reports_commit_hook_failures_without_creating_a_commit() {
        use std::os::unix::fs::PermissionsExt;

        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repository(&git, dir.path());
        commit_file(&git, dir.path(), "initial\n", "initial");
        std::fs::write(dir.path().join("file.txt"), "changed\n").unwrap();
        ensure_success(
            git.run(dir.path(), &strings(&["add", "--", "file.txt"]), None)
                .unwrap(),
        )
        .unwrap();
        let hook = dir.path().join(".git/hooks/pre-commit");
        std::fs::write(&hook, "#!/bin/sh\necho hook failed >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let before = git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        let output = git
            .run(dir.path(), &strings(&["commit", "-m", "blocked"]), None)
            .unwrap();
        assert!(!output.status.success());
        assert!(error_text(&output).contains("hook failed"));
        assert_eq!(
            git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap(),
            before
        );
    }

    #[test]
    fn linked_worktrees_share_a_common_git_directory() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let linked = dir.path().join("linked");
        let main = dir.path().join("main");
        std::fs::create_dir(&main).unwrap();
        init_repository(&git, &main);
        commit_file(&git, &main, "initial\n", "initial");
        ensure_success(
            git.run(
                &main,
                &[
                    "worktree".into(),
                    "add".into(),
                    linked.to_string_lossy().into(),
                ],
                None,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            git.inspect_repository(&main).unwrap().common_git_dir,
            git.inspect_repository(&linked).unwrap().common_git_dir
        );
    }

    #[test]
    fn merge_and_cherry_pick_conflicts_can_be_aborted() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repository(&git, dir.path());
        commit_file(&git, dir.path(), "base\n", "base");
        ensure_success(
            git.run(dir.path(), &strings(&["switch", "-c", "feature"]), None)
                .unwrap(),
        )
        .unwrap();
        let feature = commit_file(&git, dir.path(), "feature\n", "feature");
        ensure_success(
            git.run(dir.path(), &strings(&["switch", "main"]), None)
                .unwrap(),
        )
        .unwrap();
        commit_file(&git, dir.path(), "main\n", "main");

        assert!(!git
            .run(dir.path(), &strings(&["merge", "feature"]), None)
            .unwrap()
            .status
            .success());
        assert!(dir.path().join(".git/MERGE_HEAD").exists());
        ensure_success(
            git.run(dir.path(), &strings(&["merge", "--abort"]), None)
                .unwrap(),
        )
        .unwrap();

        assert!(!git
            .run(dir.path(), &strings(&["cherry-pick", &feature]), None,)
            .unwrap()
            .status
            .success());
        assert!(dir.path().join(".git/CHERRY_PICK_HEAD").exists());
        ensure_success(
            git.run(dir.path(), &strings(&["cherry-pick", "--abort"]), None)
                .unwrap(),
        )
        .unwrap();
        assert!(!dir.path().join(".git/CHERRY_PICK_HEAD").exists());

        assert!(!git
            .run(dir.path(), &strings(&["rebase", "feature"]), None)
            .unwrap()
            .status
            .success());
        assert!(
            dir.path().join(".git/rebase-merge").exists()
                || dir.path().join(".git/rebase-apply").exists()
        );
        ensure_success(
            git.run(dir.path(), &strings(&["rebase", "--abort"]), None)
                .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn rebase_conflict_can_be_aborted() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repository(&git, dir.path());
        commit_file(&git, dir.path(), "base\n", "base");
        ensure_success(
            git.run(dir.path(), &strings(&["switch", "-c", "feature"]), None)
                .unwrap(),
        )
        .unwrap();
        commit_file(&git, dir.path(), "feature\n", "feature");
        ensure_success(
            git.run(dir.path(), &strings(&["switch", "main"]), None)
                .unwrap(),
        )
        .unwrap();
        commit_file(&git, dir.path(), "main\n", "main");
        let output = git
            .run(dir.path(), &strings(&["rebase", "feature"]), None)
            .unwrap();
        assert!(!output.status.success());
        let inspection = git.inspect_repository(dir.path()).unwrap();
        assert_eq!(
            ongoing_state(&inspection.git_dir).unwrap().kind,
            OngoingKind::Rebase
        );
        ensure_success(
            git.run(dir.path(), &strings(&["rebase", "--abort"]), None)
                .unwrap(),
        )
        .unwrap();
        assert!(ongoing_state(&inspection.git_dir).is_none());
    }

    #[test]
    fn initializes_and_reads_a_direct_submodule() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("child");
        let parent = dir.path().join("parent");
        std::fs::create_dir(&child).unwrap();
        std::fs::create_dir(&parent).unwrap();
        init_repository(&git, &child);
        commit_file(&git, &child, "child\n", "child");
        init_repository(&git, &parent);
        commit_file(&git, &parent, "parent\n", "parent");
        ensure_success(
            git.run(
                &parent,
                &[
                    "-c".into(),
                    "protocol.file.allow=always".into(),
                    "submodule".into(),
                    "add".into(),
                    child.to_string_lossy().into(),
                    "deps/child".into(),
                ],
                None,
            )
            .unwrap(),
        )
        .unwrap();
        ensure_success(
            git.run(
                &parent,
                &strings(&["submodule", "sync", "--", "deps/child"]),
                None,
            )
            .unwrap(),
        )
        .unwrap();
        let modules = git.submodules(&parent).unwrap();
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].path, "deps/child");
        assert!(modules[0].initialized);
    }

    #[test]
    fn force_with_lease_rejects_a_remote_race() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let remote = dir.path().join("remote.git");
        let seed = dir.path().join("seed");
        let first = dir.path().join("first");
        let stale = dir.path().join("stale");
        std::fs::create_dir(&remote).unwrap();
        std::fs::create_dir(&seed).unwrap();
        ensure_success(
            git.run(&remote, &strings(&["init", "--bare"]), None)
                .unwrap(),
        )
        .unwrap();
        init_repository(&git, &seed);
        commit_file(&git, &seed, "base\n", "base");
        ensure_success(
            git.run(
                &seed,
                &[
                    "remote".into(),
                    "add".into(),
                    "origin".into(),
                    remote.to_string_lossy().into(),
                ],
                None,
            )
            .unwrap(),
        )
        .unwrap();
        ensure_success(
            git.run(&seed, &strings(&["push", "-u", "origin", "main"]), None)
                .unwrap(),
        )
        .unwrap();
        ensure_success(
            git.run(
                &remote,
                &strings(&["symbolic-ref", "HEAD", "refs/heads/main"]),
                None,
            )
            .unwrap(),
        )
        .unwrap();
        for target in [&first, &stale] {
            ensure_success(
                git.run(
                    dir.path(),
                    &[
                        "clone".into(),
                        remote.to_string_lossy().into(),
                        target.to_string_lossy().into(),
                    ],
                    None,
                )
                .unwrap(),
            )
            .unwrap();
            for (key, value) in [
                ("user.name", "GitDock Test"),
                ("user.email", "test@gitdock.local"),
            ] {
                ensure_success(
                    git.run(target, &strings(&["config", key, value]), None)
                        .unwrap(),
                )
                .unwrap();
            }
        }
        let expected = git
            .text(&stale, &["rev-parse", "refs/remotes/origin/main"])
            .unwrap();
        commit_file(&git, &first, "first\n", "first");
        ensure_success(
            git.run(&first, &strings(&["push", "origin", "main"]), None)
                .unwrap(),
        )
        .unwrap();
        commit_file(&git, &stale, "stale\n", "stale");
        let output = git
            .run(
                &stale,
                &[
                    "push".into(),
                    format!("--force-with-lease=refs/heads/main:{expected}"),
                    "origin".into(),
                    "HEAD:refs/heads/main".into(),
                ],
                None,
            )
            .unwrap();
        assert!(!output.status.success());
    }
}
