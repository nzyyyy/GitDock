use crate::models::*;
use std::{
    ffi::OsStr,
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
        self.run_env(cwd, args, input, &[])
    }

    pub fn run_env(
        &self,
        cwd: &Path,
        args: &[String],
        input: Option<&[u8]>,
        env: &[(String, String)],
    ) -> Result<Output, String> {
        let mut command = Command::new(&self.path);
        command
            .current_dir(cwd)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in env {
            command.env(key, value);
        }
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
            "--exclude=refs/stash".into(),
            "--all".into(),
            "--date-order".into(),
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

    pub fn rebase_commits(&self, cwd: &Path, onto: &str) -> Result<Vec<RebaseCommit>, String> {
        let output = self.run(
            cwd,
            &[
                "log".into(),
                "--reverse".into(),
                "--format=%H%x1f%s%x1f%an%x1e".into(),
                format!("{onto}..HEAD"),
            ],
            None,
        )?;
        if !output.status.success() {
            return Err(error_text(&output));
        }
        Ok(parse_rebase_commits(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }

    pub fn file_history(&self, cwd: &Path, path: &str) -> Result<Vec<FileHistoryEntry>, String> {
        let output = self.run(
            cwd,
            &[
                "log".into(),
                "--follow".into(),
                "-n".into(),
                "500".into(),
                "--format=%H%x1f%an%x1f%aI%x1f%s%x1e".into(),
                "--".into(),
                path.into(),
            ],
            None,
        )?;
        if !output.status.success() {
            return Err(error_text(&output));
        }
        Ok(parse_file_history(&String::from_utf8_lossy(&output.stdout)))
    }

    pub fn commit_detail(&self, cwd: &Path, oid: &str) -> Result<CommitDetail, String> {
        let output = ensure_success(self.run(
            cwd,
            &[
                "show".into(),
                "--first-parent".into(),
                "--no-patch".into(),
                "--numstat".into(),
                "--format=%H%x1f%an%x1f%ae%x1f%aI%x1f%B%x1e".into(),
                oid.into(),
                "--".into(),
            ],
            None,
        )?)?;
        parse_commit_detail(&String::from_utf8_lossy(&output.stdout))
    }

    pub fn commit_file_diff(&self, cwd: &Path, oid: &str, path: &str) -> Result<String, String> {
        limited_patch(self.run(
            cwd,
            &[
                "show".into(),
                "--format=".into(),
                "--no-ext-diff".into(),
                "--no-color".into(),
                oid.into(),
                "--".into(),
                path.into(),
            ],
            None,
        )?)
    }

    pub fn stash_detail(&self, cwd: &Path, oid: &str) -> Result<CommitDetail, String> {
        let (_, untracked_parent) = self.stash_parents(cwd, oid)?;
        let mut detail = self.commit_detail(cwd, oid)?;
        if let Some(parent) = untracked_parent {
            let output = ensure_success(self.run(
                cwd,
                &[
                    "show".into(),
                    "--root".into(),
                    "--format=".into(),
                    "--numstat".into(),
                    parent,
                    "--".into(),
                ],
                None,
            )?)?;
            detail.files.extend(
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .filter_map(parse_numstat_line),
            );
        }
        Ok(detail)
    }

    pub fn stash_file_diff(&self, cwd: &Path, oid: &str, path: &str) -> Result<String, String> {
        let (base, untracked_parent) = self.stash_parents(cwd, oid)?;
        let tracked = limited_patch(self.run(
            cwd,
            &[
                "diff".into(),
                "--no-ext-diff".into(),
                "--no-color".into(),
                base,
                oid.into(),
                "--".into(),
                path.into(),
            ],
            None,
        )?)?;
        if !tracked.is_empty() {
            return Ok(tracked);
        }
        let Some(parent) = untracked_parent else {
            return Ok(tracked);
        };
        limited_patch(self.run(
            cwd,
            &[
                "show".into(),
                "--root".into(),
                "--format=".into(),
                "--no-ext-diff".into(),
                "--no-color".into(),
                parent,
                "--".into(),
                path.into(),
            ],
            None,
        )?)
    }

    fn stash_parents(&self, cwd: &Path, oid: &str) -> Result<(String, Option<String>), String> {
        let value = self.text(cwd, &["rev-list", "--parents", "-n", "1", oid])?;
        let parents: Vec<_> = value.split_whitespace().skip(1).collect();
        if parents.len() < 2 {
            return Err("Selected object is not a stash".into());
        }
        Ok((
            parents[0].into(),
            parents.get(2).map(|parent| (*parent).into()),
        ))
    }

    pub fn blame(&self, cwd: &Path, path: &str) -> Result<BlameFile, String> {
        let output = self.run(cwd, &strings(&["blame", "--porcelain", "--", path]), None)?;
        if !output.status.success() {
            return Err(error_text(&output));
        }
        let text = String::from_utf8_lossy(&output.stdout).to_string();
        if text.len() > MAX_DIFF_BYTES || text.lines().count() > MAX_DIFF_LINES {
            return Err("This file is too large for blame".into());
        }
        parse_blame(path, &text)
    }

    pub fn branches(&self, cwd: &Path) -> Result<Vec<BranchInfo>, String> {
        let output = self.run(
            cwd,
            &strings(&[
                "for-each-ref",
                "--format=%(refname)%09%(objectname)%09%(HEAD)%09%(upstream:short)",
                "refs/heads",
                "refs/remotes",
            ]),
            None,
        )?;
        if !output.status.success() {
            return Err(error_text(&output));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let mut p = line.split('\t');
                let full = p.next()?;
                let oid = p.next()?.into();
                let current = p.next() == Some("*");
                let upstream = p.next().filter(|s| !s.is_empty()).map(Into::into);
                let remote = full.starts_with("refs/remotes/");
                let name = full.trim_start_matches(if remote {
                    "refs/remotes/"
                } else {
                    "refs/heads/"
                });
                if remote && name.ends_with("/HEAD") {
                    return None;
                }
                Some(BranchInfo {
                    name: name.into(),
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
        names
            .lines()
            .filter(|name| !name.trim().is_empty())
            .map(|name| {
                let fetch = self.remote_urls(cwd, name, "url")?;
                let configured_push = self.remote_urls(cwd, name, "pushurl")?;
                let push = if configured_push.is_empty() {
                    fetch.clone()
                } else {
                    configured_push
                };
                Ok(RemoteInfo {
                    name: name.into(),
                    fetch_urls: fetch.iter().map(|url| redact_url(url)).collect(),
                    push_urls: push.iter().map(|url| redact_url(url)).collect(),
                })
            })
            .collect()
    }

    fn remote_urls(&self, cwd: &Path, name: &str, key: &str) -> Result<Vec<String>, String> {
        let args = strings(&["config", "--get-all", &format!("remote.{name}.{key}")]);
        let output = self.run(cwd, &args, None)?;
        if !output.status.success() && !output.stderr.is_empty() {
            return Err(error_text(&output));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
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

fn parse_file_history(text: &str) -> Vec<FileHistoryEntry> {
    text.split('\x1e')
        .filter_map(|record| {
            let record = record.trim_matches('\n');
            if record.is_empty() {
                return None;
            }
            let mut p = record.splitn(4, '\x1f');
            Some(FileHistoryEntry {
                oid: p.next()?.into(),
                author: p.next()?.into(),
                authored_at: p.next()?.into(),
                subject: p.next().unwrap_or_default().into(),
            })
        })
        .collect()
}

fn parse_blame(path: &str, text: &str) -> Result<BlameFile, String> {
    let mut content = Vec::new();
    let mut hunks: Vec<BlameHunk> = Vec::new();
    let mut current: Option<BlameHunk> = None;

    for line in text.lines() {
        if let Some(code) = line.strip_prefix('\t') {
            content.push(code.to_string());
            if let Some(hunk) = current.as_mut() {
                hunk.line_count += 1;
            }
            continue;
        }
        if line.len() >= 40
            && line.as_bytes().get(40).is_some_and(|b| *b == b' ')
            && line[..40].bytes().all(|b| b.is_ascii_hexdigit())
        {
            if let Some(previous) = current.take() {
                hunks.push(previous);
            }
            let final_line = line[41..]
                .split_whitespace()
                .nth(1)
                .and_then(|v| v.parse().ok())
                .ok_or("Invalid blame header")?;
            current = Some(BlameHunk {
                oid: line[..40].to_string(),
                author: String::new(),
                author_time: 0,
                start_line: final_line,
                line_count: 0,
            });
            continue;
        }
        if let Some(hunk) = current.as_mut() {
            if let Some(name) = line.strip_prefix("author ") {
                hunk.author = name.to_string();
            } else if let Some(time) = line.strip_prefix("author-time ") {
                hunk.author_time = time.parse().unwrap_or(0);
            }
        }
    }
    if let Some(previous) = current {
        hunks.push(previous);
    }
    Ok(BlameFile {
        path: path.to_string(),
        content,
        hunks,
    })
}

fn parse_rebase_commits(text: &str) -> Vec<RebaseCommit> {
    text.split('\x1e')
        .filter_map(|record| {
            let record = record.trim_matches('\n');
            if record.is_empty() {
                return None;
            }
            let mut p = record.splitn(3, '\x1f');
            Some(RebaseCommit {
                oid: p.next()?.into(),
                subject: p.next()?.into(),
                author: p.next().unwrap_or_default().into(),
            })
        })
        .collect()
}

fn parse_commit_detail(text: &str) -> Result<CommitDetail, String> {
    let (header, stats) = text.split_once('\x1e').ok_or("Invalid commit detail")?;
    let mut fields = header.trim_start_matches('\n').splitn(5, '\x1f');
    Ok(CommitDetail {
        oid: fields
            .next()
            .filter(|value| !value.is_empty())
            .ok_or("Commit id is missing")?
            .into(),
        author: fields.next().ok_or("Commit author is missing")?.into(),
        email: fields.next().ok_or("Commit email is missing")?.into(),
        authored_at: fields.next().ok_or("Commit date is missing")?.into(),
        message: fields.next().unwrap_or_default().trim_end().into(),
        files: stats.lines().filter_map(parse_numstat_line).collect(),
    })
}

pub(crate) fn parse_numstat_line(line: &str) -> Option<CommitFileChange> {
    let mut fields = line.splitn(3, '\t');
    let additions = parse_numstat_count(fields.next()?)?;
    let deletions = parse_numstat_count(fields.next()?)?;
    let (path, original_path) = parse_numstat_path(fields.next()?.trim());
    if path.is_empty() {
        return None;
    }
    Some(CommitFileChange {
        path,
        original_path,
        additions,
        deletions,
    })
}

fn parse_numstat_count(value: &str) -> Option<Option<u32>> {
    if value == "-" {
        Some(None)
    } else {
        value.parse().ok().map(Some)
    }
}

fn parse_numstat_path(path: &str) -> (String, Option<String>) {
    if let Some(start) = path.find('{') {
        if let Some(arrow) = path[start..].find(" => ") {
            let old_end = start + arrow;
            let new_start = old_end + 4;
            if let Some(close) = path[new_start..].find('}') {
                let prefix = &path[..start];
                let old = &path[start + 1..old_end];
                let new = &path[new_start..new_start + close];
                let suffix = &path[new_start + close + 1..];
                return (
                    format!("{prefix}{new}{suffix}"),
                    Some(format!("{prefix}{old}{suffix}")),
                );
            }
        }
    }
    match path.split_once(" => ") {
        Some((old, new)) => (new.into(), Some(old.into())),
        None => (path.into(), None),
    }
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
                active.push(commit.oid.clone());
                active.len() - 1
            });
        active[column].clear();
        let mut parent_columns = Vec::with_capacity(commit.parents.len());
        for (offset, parent) in commit.parents.iter().enumerate() {
            let parent_column = match active.iter().position(|oid| oid == parent) {
                Some(existing) if offset == 0 && existing > column => {
                    active[existing].clear();
                    active[column] = parent.clone();
                    column
                }
                Some(existing) => existing,
                None if offset == 0 => {
                    active[column] = parent.clone();
                    column
                }
                None => {
                    let target = active
                        .iter()
                        .enumerate()
                        .skip(column + 1)
                        .find_map(|(index, oid)| oid.is_empty().then_some(index))
                        .unwrap_or_else(|| {
                            active.push(String::new());
                            active.len() - 1
                        });
                    active[target] = parent.clone();
                    target
                }
            };
            parent_columns.push(parent_column);
        }
        while active.last().is_some_and(String::is_empty) {
            active.pop();
        }
        commit.lane = GraphLane {
            column,
            parent_columns,
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

pub(crate) fn capabilities(kind: RepositoryKind) -> RepositoryCapabilities {
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

fn limited_patch(output: Output) -> Result<String, String> {
    let output = ensure_success(output)?;
    let patch = String::from_utf8_lossy(&output.stdout).to_string();
    if patch.len() > MAX_DIFF_BYTES || patch.lines().count() > MAX_DIFF_LINES {
        return Err("This file diff is too large".into());
    }
    Ok(patch)
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
    use crate::working_tree;

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
        commit_file_dated(git, path, contents, message, None)
    }

    fn commit_file_dated(
        git: &Git,
        path: &Path,
        contents: &str,
        message: &str,
        date: Option<&str>,
    ) -> String {
        std::fs::write(path.join("file.txt"), contents).unwrap();
        ensure_success(
            git.run(path, &strings(&["add", "--", "file.txt"]), None)
                .unwrap(),
        )
        .unwrap();
        let env = date
            .map(|date| {
                vec![
                    ("GIT_AUTHOR_DATE".into(), date.into()),
                    ("GIT_COMMITTER_DATE".into(), date.into()),
                ]
            })
            .unwrap_or_default();
        ensure_success(
            git.run_env(path, &strings(&["commit", "-m", message]), None, &env)
                .unwrap(),
        )
        .unwrap();
        git.text(path, &["rev-parse", "HEAD"]).unwrap()
    }

    fn graph_commit(oid: &str, parents: &[&str]) -> CommitInfo {
        CommitInfo {
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
        }
    }

    #[test]
    fn remotes_list_every_configured_url() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repository(&git, dir.path());
        for args in [
            vec!["remote", "add", "origin", "../remote.git"],
            vec![
                "remote",
                "set-url",
                "--add",
                "origin",
                "https://extra.example.com/repo.git",
            ],
        ] {
            ensure_success(git.run(dir.path(), &strings(&args), None).unwrap()).unwrap();
        }

        let remotes = git.remotes(dir.path()).unwrap();
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "origin");
        let fetch_urls = vec![
            "../remote.git".to_string(),
            "https://extra.example.com/repo.git".to_string(),
        ];
        assert_eq!(remotes[0].fetch_urls, fetch_urls);
        // Without a configured pushurl, push falls back to the fetch URLs.
        assert_eq!(remotes[0].push_urls, fetch_urls);

        for url in [
            "https://push1.example.com/repo.git",
            "https://push2.example.com/repo.git",
        ] {
            ensure_success(
                git.run(
                    dir.path(),
                    &strings(&["remote", "set-url", "--add", "--push", "origin", url]),
                    None,
                )
                .unwrap(),
            )
            .unwrap();
        }

        let remotes = git.remotes(dir.path()).unwrap();
        assert_eq!(remotes[0].fetch_urls, fetch_urls);
        assert_eq!(
            remotes[0].push_urls,
            vec![
                "https://push1.example.com/repo.git".to_string(),
                "https://push2.example.com/repo.git".to_string(),
            ]
        );
    }

    #[test]
    fn parses_commit_detail_numstat() {
        let detail = parse_commit_detail(
            "aaaaaaaa\x1fAda\x1fada@example.com\x1f2026-08-09T00:00:00Z\x1fSelected\n\nBody line\n\x1e\n12\t3\tsrc/a.txt\n-\t-\timage.png\n1\t2\told.ts => new.ts\n4\t0\tsrc/{left.rs => right.rs}\n",
        )
        .unwrap();
        assert_eq!(detail.oid, "aaaaaaaa");
        assert_eq!(detail.author, "Ada");
        assert_eq!(detail.email, "ada@example.com");
        assert_eq!(detail.authored_at, "2026-08-09T00:00:00Z");
        assert_eq!(detail.message, "Selected\n\nBody line");
        assert_eq!(
            detail.files,
            vec![
                CommitFileChange {
                    path: "src/a.txt".into(),
                    original_path: None,
                    additions: Some(12),
                    deletions: Some(3),
                },
                CommitFileChange {
                    path: "image.png".into(),
                    original_path: None,
                    additions: None,
                    deletions: None,
                },
                CommitFileChange {
                    path: "new.ts".into(),
                    original_path: Some("old.ts".into()),
                    additions: Some(1),
                    deletions: Some(2),
                },
                CommitFileChange {
                    path: "src/right.rs".into(),
                    original_path: Some("src/left.rs".into()),
                    additions: Some(4),
                    deletions: Some(0),
                },
            ]
        );

        let empty = parse_commit_detail(
            "bbbbbbbb\x1fLin\x1flin@example.com\x1f2026-08-08T00:00:00Z\x1fEmpty\n\x1e\n",
        )
        .unwrap();
        assert!(empty.files.is_empty());
        assert_eq!(empty.message, "Empty");
    }

    #[test]
    fn commit_detail_reads_git_show_numstat() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repository(&git, dir.path());
        commit_file(&git, dir.path(), "base\n", "base");
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.txt"), "hello\n").unwrap();
        std::fs::write(dir.path().join("image.bin"), [0u8, 1, 2, 255]).unwrap();
        std::fs::write(dir.path().join("old.ts"), "one\n").unwrap();
        ensure_success(
            git.run(
                dir.path(),
                &strings(&["add", "--", "src/a.txt", "image.bin", "old.ts"]),
                None,
            )
            .unwrap(),
        )
        .unwrap();
        ensure_success(
            git.run(
                dir.path(),
                &strings(&["commit", "-m", "files\n\nBody"]),
                None,
            )
            .unwrap(),
        )
        .unwrap();
        ensure_success(
            git.run(dir.path(), &strings(&["mv", "old.ts", "new.ts"]), None)
                .unwrap(),
        )
        .unwrap();
        ensure_success(
            git.run(dir.path(), &strings(&["commit", "-m", "rename"]), None)
                .unwrap(),
        )
        .unwrap();
        let oid = git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        let renamed = git.commit_detail(dir.path(), &oid).unwrap();
        assert_eq!(renamed.message, "rename");
        assert_eq!(renamed.files.len(), 1);
        assert_eq!(renamed.files[0].path, "new.ts");
        assert_eq!(renamed.files[0].original_path.as_deref(), Some("old.ts"));

        let parent = git.text(dir.path(), &["rev-parse", "HEAD^"]).unwrap();
        let files = git.commit_detail(dir.path(), &parent).unwrap();
        assert_eq!(files.message, "files\n\nBody");
        assert_eq!(files.author, "GitDock Test");
        assert_eq!(files.email, "test@gitdock.local");
        let paths: Vec<_> = files.files.iter().map(|file| file.path.as_str()).collect();
        assert!(paths.contains(&"src/a.txt"));
        assert!(paths.contains(&"image.bin"));
        assert!(paths.contains(&"old.ts"));
        let text = files
            .files
            .iter()
            .find(|file| file.path == "src/a.txt")
            .unwrap();
        assert_eq!(text.additions, Some(1));
        assert_eq!(text.deletions, Some(0));
        let binary = files
            .files
            .iter()
            .find(|file| file.path == "image.bin")
            .unwrap();
        assert_eq!(binary.additions, None);
        assert_eq!(binary.deletions, None);

        ensure_success(
            git.run(
                dir.path(),
                &strings(&["commit", "--allow-empty", "-m", "empty"]),
                None,
            )
            .unwrap(),
        )
        .unwrap();
        let empty_oid = git.text(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        let empty = git.commit_detail(dir.path(), &empty_oid).unwrap();
        assert_eq!(empty.message, "empty");
        assert!(empty.files.is_empty());
    }

    #[test]
    fn stash_detail_and_file_diff_include_untracked_files() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repository(&git, dir.path());
        commit_file(&git, dir.path(), "base\n", "base");

        std::fs::write(dir.path().join("file.txt"), "staged\n").unwrap();
        ensure_success(
            git.run(dir.path(), &strings(&["add", "--", "file.txt"]), None)
                .unwrap(),
        )
        .unwrap();
        std::fs::write(dir.path().join("file.txt"), "unstaged\n").unwrap();
        std::fs::write(dir.path().join("new.txt"), "untracked\n").unwrap();
        ensure_success(
            git.run(
                dir.path(),
                &strings(&["stash", "push", "--include-untracked", "-m", "complete"]),
                None,
            )
            .unwrap(),
        )
        .unwrap();

        let oid = git.text(dir.path(), &["rev-parse", "refs/stash"]).unwrap();
        let detail = git.stash_detail(dir.path(), &oid).unwrap();
        assert_eq!(detail.message, "On main: complete");
        assert_eq!(detail.files.len(), 2);
        assert!(detail.files.iter().any(|file| file.path == "file.txt"));
        assert!(detail.files.iter().any(|file| file.path == "new.txt"));

        let tracked = git.stash_file_diff(dir.path(), &oid, "file.txt").unwrap();
        assert!(tracked.contains("-base"));
        assert!(tracked.contains("+unstaged"));
        let untracked = git.stash_file_diff(dir.path(), &oid, "new.txt").unwrap();
        assert!(untracked.contains("new file mode"));
        assert!(untracked.contains("+untracked"));

        std::fs::write(dir.path().join("file.txt"), "tracked only\n").unwrap();
        ensure_success(
            git.run(
                dir.path(),
                &strings(&["stash", "push", "-m", "tracked-only"]),
                None,
            )
            .unwrap(),
        )
        .unwrap();
        let oid = git.text(dir.path(), &["rev-parse", "refs/stash"]).unwrap();
        let detail = git.stash_detail(dir.path(), &oid).unwrap();
        assert_eq!(detail.files.len(), 1);
        assert_eq!(detail.files[0].path, "file.txt");
        assert!(git
            .stash_file_diff(dir.path(), &oid, "file.txt")
            .unwrap()
            .contains("+tracked only"));
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
    fn lanes_continue_across_pages_without_reusing_live_branch_columns() {
        let mut first = vec![
            graph_commit("merge", &["a", "b", "c"]),
            graph_commit("a", &["d"]),
        ];
        let active = assign_lanes(&mut first, Vec::new());
        assert_eq!(first[0].lane.parent_columns, [0, 1, 2]);
        assert_eq!(active, ["d", "b", "c"]);

        let mut second = vec![
            graph_commit("b", &["d"]),
            graph_commit("c", &["d"]),
            graph_commit("d", &[]),
        ];
        let active = assign_lanes(&mut second, active);
        assert_eq!(
            second
                .iter()
                .map(|item| item.lane.column)
                .collect::<Vec<_>>(),
            [1, 2, 0]
        );
        assert!(active.is_empty());
    }

    #[test]
    fn new_ref_tip_uses_separate_lane_until_shared_parent() {
        let mut first = vec![graph_commit("main", &["base"])];
        let active = assign_lanes(&mut first, Vec::new());
        assert_eq!(first[0].lane.column, 0);
        assert_eq!(first[0].lane.parent_columns, [0]);

        let mut second = vec![
            graph_commit("feature", &["feature-work"]),
            graph_commit("feature-work", &["base"]),
            graph_commit("base", &[]),
        ];
        let active = assign_lanes(&mut second, active);
        assert_eq!(
            second
                .iter()
                .map(|item| item.lane.column)
                .collect::<Vec<_>>(),
            [1, 1, 0]
        );
        assert_eq!(second[0].lane.parent_columns, [1]);
        assert_eq!(second[1].lane.parent_columns, [0]);
        assert!(active.is_empty());
    }

    #[test]
    fn nested_merges_keep_main_and_branch_lanes_stable() {
        let mut first = vec![
            graph_commit("22c26cc", &["439f0b7", "1da80e7"]),
            graph_commit("1da80e7", &["0a17b1e"]),
            graph_commit("0a17b1e", &["e9fe219"]),
            graph_commit("e9fe219", &["439f0b7", "70c0b0e"]),
        ];
        let active = assign_lanes(&mut first, Vec::new());
        assert_eq!(
            first
                .iter()
                .map(|item| item.lane.column)
                .collect::<Vec<_>>(),
            [0, 1, 1, 1]
        );
        assert_eq!(first[3].lane.parent_columns, [0, 2]);
        assert_eq!(active, ["439f0b7", "", "70c0b0e"]);

        let mut second = vec![
            graph_commit("70c0b0e", &["20a3267"]),
            graph_commit("20a3267", &["d025256", "e78603f"]),
            graph_commit("d025256", &["bdca1c6"]),
            graph_commit("bdca1c6", &["bcd6871"]),
            graph_commit("bcd6871", &["a3b47e7"]),
            graph_commit("439f0b7", &["e78603f"]),
            graph_commit("e78603f", &["a3b47e7"]),
            graph_commit("a3b47e7", &[]),
        ];
        let active = assign_lanes(&mut second, active);
        assert_eq!(
            second
                .iter()
                .map(|item| item.lane.column)
                .collect::<Vec<_>>(),
            [2, 2, 2, 2, 2, 0, 0, 0]
        );
        assert_eq!(second[1].lane.parent_columns, [2, 3]);
        assert_eq!(second[5].lane.parent_columns, [0]);
        assert_eq!(second[6].lane.parent_columns, [0]);
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

        let untracked = working_tree::read_snapshot(&git, 1, dir.path(), false, 1).unwrap();
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
        let staged = working_tree::read_snapshot(&git, 1, dir.path(), false, 2).unwrap();
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
        let diff = working_tree::read_diff(&git, dir.path(), "hello world.txt", false, 3).unwrap();
        assert!(!diff.hunks.is_empty());
        std::fs::write(
            dir.path().join("hello world.txt"),
            "changed after snapshot\n",
        )
        .unwrap();
        assert_ne!(
            working_tree::read_diff(&git, dir.path(), "hello world.txt", false, 3)
                .unwrap()
                .patch,
            diff.patch
        );
        let history = git.history(dir.path(), None, 20).unwrap();
        assert_eq!(history.commits[0].subject, "initial");
    }

    #[test]
    fn history_orders_parallel_branches_by_commit_date() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repository(&git, dir.path());
        commit_file_dated(
            &git,
            dir.path(),
            "base\n",
            "base",
            Some("2026-08-17T00:00:00 +0000"),
        );
        ensure_success(
            git.run(dir.path(), &strings(&["checkout", "-b", "feature"]), None)
                .unwrap(),
        )
        .unwrap();
        commit_file_dated(
            &git,
            dir.path(),
            "feature\n",
            "feature-old",
            Some("2026-08-24T00:00:00 +0000"),
        );
        ensure_success(
            git.run(dir.path(), &strings(&["checkout", "main"]), None)
                .unwrap(),
        )
        .unwrap();
        commit_file_dated(
            &git,
            dir.path(),
            "main\n",
            "main-new",
            Some("2026-08-25T00:00:00 +0000"),
        );

        let history = git.history(dir.path(), None, 20).unwrap();
        let subjects: Vec<_> = history
            .commits
            .iter()
            .map(|commit| commit.subject.as_str())
            .collect();
        let main = subjects
            .iter()
            .position(|subject| *subject == "main-new")
            .unwrap();
        let feature = subjects
            .iter()
            .position(|subject| *subject == "feature-old")
            .unwrap();
        assert!(
            main < feature,
            "newer main commit should appear above older parallel feature commit, got {subjects:?}"
        );
    }

    #[test]
    fn history_excludes_stash_commits() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repository(&git, dir.path());
        let initial = commit_file(&git, dir.path(), "initial\n", "initial");
        std::fs::write(dir.path().join("file.txt"), "changed\n").unwrap();
        std::fs::write(dir.path().join("untracked.txt"), "untracked\n").unwrap();
        ensure_success(
            git.run(
                dir.path(),
                &strings(&["stash", "push", "--include-untracked", "-m", "hidden"]),
                None,
            )
            .unwrap(),
        )
        .unwrap();

        let excluded = [
            git.text(dir.path(), &["rev-parse", "refs/stash"]).unwrap(),
            git.text(dir.path(), &["rev-parse", "refs/stash^2"])
                .unwrap(),
            git.text(dir.path(), &["rev-parse", "refs/stash^3"])
                .unwrap(),
        ];
        let history = git.history(dir.path(), None, 20).unwrap();

        assert_eq!(
            history
                .commits
                .iter()
                .map(|commit| commit.oid.as_str())
                .collect::<Vec<_>>(),
            vec![initial.as_str()]
        );
        assert!(history
            .commits
            .iter()
            .all(|commit| !excluded.contains(&commit.oid)));
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
    fn listing_branches_keeps_created_branch_after_switch() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repository(&git, dir.path());
        commit_file(&git, dir.path(), "base\n", "base");
        ensure_success(
            git.run(dir.path(), &strings(&["switch", "-c", "test"]), None)
                .unwrap(),
        )
        .unwrap();
        ensure_success(
            git.run(dir.path(), &strings(&["switch", "main"]), None)
                .unwrap(),
        )
        .unwrap();
        let branches = git.branches(dir.path()).unwrap();
        let test = branches
            .iter()
            .find(|branch| !branch.remote && branch.name == "test")
            .unwrap();
        let main = branches
            .iter()
            .find(|branch| !branch.remote && branch.name == "main")
            .unwrap();
        assert!(!test.current);
        assert!(main.current);
    }

    #[test]
    fn listing_branches_omits_remote_head() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let remote = dir.path().join("remote.git");
        let repo = dir.path().join("repo");
        std::fs::create_dir(&remote).unwrap();
        std::fs::create_dir(&repo).unwrap();
        ensure_success(
            git.run(&remote, &strings(&["init", "--bare"]), None)
                .unwrap(),
        )
        .unwrap();
        init_repository(&git, &repo);
        commit_file(&git, &repo, "base\n", "base");
        ensure_success(
            git.run(
                &repo,
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
            git.run(&repo, &strings(&["push", "-u", "origin", "main"]), None)
                .unwrap(),
        )
        .unwrap();
        ensure_success(
            git.run(
                &repo,
                &strings(&["remote", "set-head", "origin", "main"]),
                None,
            )
            .unwrap(),
        )
        .unwrap();
        let branches = git.branches(&repo).unwrap();
        assert!(branches
            .iter()
            .any(|branch| branch.remote && branch.name == "origin/main"));
        assert!(branches.iter().all(|branch| branch.name != "origin/HEAD"));
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

    #[test]
    fn file_history_lists_commits_for_a_path() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repository(&git, dir.path());
        commit_file(&git, dir.path(), "one\n", "one");
        let second = commit_file(&git, dir.path(), "two\n", "two");

        let entries = git.file_history(dir.path(), "file.txt").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].oid, second);
        assert_eq!(entries[0].subject, "two");
        assert_eq!(entries[1].subject, "one");
    }

    #[test]
    fn blame_attributes_each_line_to_a_commit() {
        let git = Git::discover(None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        init_repository(&git, dir.path());
        let first = commit_file(&git, dir.path(), "a\nb\n", "first");
        let second = commit_file(&git, dir.path(), "a\nc\n", "second");

        let blame = git.blame(dir.path(), "file.txt").unwrap();
        assert_eq!(blame.content, vec!["a".to_string(), "c".to_string()]);
        assert_eq!(blame.hunks.len(), 2);
        assert_eq!(blame.hunks[0].oid, first);
        assert_eq!(blame.hunks[0].start_line, 1);
        assert_eq!(blame.hunks[0].line_count, 1);
        assert_eq!(blame.hunks[1].oid, second);
        assert_eq!(blame.hunks[1].start_line, 2);
        assert_eq!(blame.hunks[1].line_count, 1);
    }
}
