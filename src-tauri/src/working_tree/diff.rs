use crate::{
    git::{error_text, parse_numstat_line, strings, Git, MAX_DIFF_BYTES, MAX_DIFF_LINES},
    models::*,
};
use std::{
    collections::{hash_map::DefaultHasher, HashMap},
    fs,
    hash::{Hash, Hasher},
    path::Path,
};

pub(super) fn status(
    git: &Git,
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
    let output = git.run(cwd, &args, None)?;
    if !output.status.success() {
        return Err(error_text(&output));
    }
    Ok(WorkingTreeSnapshot {
        id: snapshot_id,
        repository_id,
        head_oid: git.text(cwd, &["rev-parse", "--verify", "HEAD"]).ok(),
        files: parse_porcelain_v2(&output.stdout),
    })
}

pub(super) fn attach_line_stats(git: &Git, cwd: &Path, files: &mut [FileChange]) {
    if files.is_empty() {
        return;
    }
    let mut stats = HashMap::new();
    for line in numstat(git, cwd).lines() {
        if let Some(change) = parse_numstat_line(line) {
            stats.insert(change.path, (change.additions, change.deletions));
        }
    }
    for file in files {
        if file.ignored {
            continue;
        }
        if file.kind == ChangeKind::Untracked {
            let (additions, deletions) = untracked_line_stats(cwd, &file.path);
            file.additions = additions;
            file.deletions = deletions;
            continue;
        }
        if let Some((additions, deletions)) = stats.get(&file.path).copied() {
            file.additions = additions;
            file.deletions = deletions;
        }
    }
}

fn numstat(git: &Git, cwd: &Path) -> String {
    let head = git.run(cwd, &strings(&["diff", "--numstat", "HEAD", "--"]), None);
    if let Ok(output) = head {
        if output.status.success() {
            return String::from_utf8_lossy(&output.stdout).into_owned();
        }
    }
    git.run(
        cwd,
        &strings(&["diff", "--cached", "--numstat", "--"]),
        None,
    )
    .ok()
    .filter(|output| output.status.success())
    .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
    .unwrap_or_default()
}

pub(super) fn diff(
    git: &Git,
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
    let output = git.run(cwd, &args, None)?;
    if !output.status.success() {
        return Err(error_text(&output));
    }
    let mut patch = String::from_utf8_lossy(&output.stdout).to_string();
    if patch.is_empty() && !staged && cwd.join(path).is_file() {
        let output = git.run(
            cwd,
            &strings(&[
                "diff",
                "--no-ext-diff",
                "--no-color",
                "--unified=3",
                "--no-index",
                "--",
                "/dev/null",
                path,
            ]),
            None,
        )?;
        match output.status.code() {
            Some(0 | 1) => patch = String::from_utf8_lossy(&output.stdout).to_string(),
            _ => return Err(error_text(&output)),
        }
    }
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

pub(super) fn parse_porcelain_v2(bytes: &[u8]) -> Vec<FileChange> {
    let records: Vec<&[u8]> = bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .collect();
    let mut files = Vec::new();
    let mut index = 0;
    while index < records.len() {
        let record = String::from_utf8_lossy(records[index]);
        match record.as_bytes().first().copied() {
            Some(b'1') => {
                if let Some(file) = parse_ordinary(&record, 8, None) {
                    files.push(file);
                }
            }
            Some(b'2') => {
                let original = records
                    .get(index + 1)
                    .map(|record| String::from_utf8_lossy(record).into_owned());
                if let Some(file) = parse_ordinary(&record, 9, original) {
                    files.push(file);
                }
                index += 1;
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
        index += 1;
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
        additions: None,
        deletions: None,
    }
}

enum HunkLine {
    Context(String),
    Delete(String),
    Insert(String),
    NoEol,
}

fn parse_hunk_range(text: &str) -> Option<((u32, u32), &str)> {
    let digits = text
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(text.len());
    if digits == 0 {
        return None;
    }
    let start = text[..digits].parse().ok()?;
    let rest = &text[digits..];
    if let Some(rest) = rest.strip_prefix(',') {
        let count_len = rest
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(rest.len());
        if count_len == 0 {
            return None;
        }
        let count = rest[..count_len].parse().ok()?;
        Some(((start, count), &rest[count_len..]))
    } else {
        Some(((start, 1), rest))
    }
}

fn parse_hunk_header(line: &str) -> Option<(u32, u32, u32, u32)> {
    let rest = line.strip_prefix("@@")?.trim_start().strip_prefix('-')?;
    let (old, rest) = parse_hunk_range(rest)?;
    let rest = rest.trim_start().strip_prefix('+')?;
    let (new, _) = parse_hunk_range(rest)?;
    Some((old.0, old.1, new.0, new.1))
}

fn parse_hunk_line(line: &str) -> HunkLine {
    if line.starts_with('\\') {
        HunkLine::NoEol
    } else if let Some(content) = line.strip_prefix('-') {
        HunkLine::Delete(content.to_string())
    } else if let Some(content) = line.strip_prefix('+') {
        HunkLine::Insert(content.to_string())
    } else if let Some(content) = line.strip_prefix(' ') {
        HunkLine::Context(content.to_string())
    } else {
        HunkLine::Context(line.to_string())
    }
}

fn format_hunk_range(start: u32, count: u32) -> String {
    if count == 1 {
        format!("{start}")
    } else {
        format!("{start},{count}")
    }
}

fn write_hunk_lines(lines: &[HunkLine]) -> String {
    let mut body = String::new();
    for line in lines {
        match line {
            HunkLine::Context(content) => {
                body.push(' ');
                body.push_str(content);
            }
            HunkLine::Delete(content) => {
                body.push('-');
                body.push_str(content);
            }
            HunkLine::Insert(content) => {
                body.push('+');
                body.push_str(content);
            }
            HunkLine::NoEol => body.push_str("\\ No newline at end of file"),
        }
        body.push('\n');
    }
    body
}

fn number_hunk_lines(old_start: u32, new_start: u32, lines: &[HunkLine]) -> Vec<(u32, u32)> {
    let mut old = old_start;
    let mut new = new_start;
    let mut numbers = Vec::with_capacity(lines.len());
    for line in lines {
        numbers.push((old, new));
        match line {
            HunkLine::Context(_) => {
                old += 1;
                new += 1;
            }
            HunkLine::Delete(_) => old += 1,
            HunkLine::Insert(_) => new += 1,
            HunkLine::NoEol => {}
        }
    }
    numbers
}

fn change_islands(lines: &[HunkLine]) -> Vec<(usize, usize)> {
    let mut islands = Vec::new();
    let mut start = None;
    let mut end = 0;
    for (index, line) in lines.iter().enumerate() {
        match line {
            HunkLine::Delete(_) | HunkLine::Insert(_) => {
                if start.is_none() {
                    start = Some(index);
                }
                end = index;
            }
            HunkLine::NoEol => {
                if start.is_some() {
                    end = index;
                }
            }
            HunkLine::Context(_) => {
                if let Some(from) = start.take() {
                    islands.push((from, end));
                }
            }
        }
    }
    if let Some(from) = start {
        islands.push((from, end));
    }
    islands
}

fn island_span(lines: &[HunkLine], from: usize, to: usize) -> (usize, usize) {
    let start = if from > 0 && matches!(lines[from - 1], HunkLine::Context(_)) {
        from - 1
    } else {
        from
    };
    let mut end = to;
    if end + 1 < lines.len() && matches!(lines[end + 1], HunkLine::Context(_)) {
        end += 1;
        if end + 1 < lines.len() && matches!(lines[end + 1], HunkLine::NoEol) {
            end += 1;
        }
    } else if end + 1 < lines.len() && matches!(lines[end + 1], HunkLine::NoEol) {
        end += 1;
    }
    (start, end)
}

fn island_header(lines: &[HunkLine], numbers: &[(u32, u32)]) -> String {
    let old_count = lines
        .iter()
        .filter(|line| matches!(line, HunkLine::Context(_) | HunkLine::Delete(_)))
        .count() as u32;
    let new_count = lines
        .iter()
        .filter(|line| matches!(line, HunkLine::Context(_) | HunkLine::Insert(_)))
        .count() as u32;
    let old_start = lines
        .iter()
        .zip(numbers)
        .find_map(|(line, (old, _))| match line {
            HunkLine::Context(_) | HunkLine::Delete(_) => Some(*old),
            _ => None,
        })
        .unwrap_or_else(|| {
            lines
                .iter()
                .zip(numbers)
                .find_map(|(line, (old, _))| match line {
                    HunkLine::Insert(_) => Some(old.saturating_sub(1)),
                    _ => None,
                })
                .unwrap_or(0)
        });
    let new_start = lines
        .iter()
        .zip(numbers)
        .find_map(|(line, (_, new))| match line {
            HunkLine::Context(_) | HunkLine::Insert(_) => Some(*new),
            _ => None,
        })
        .unwrap_or_else(|| {
            lines
                .iter()
                .zip(numbers)
                .find_map(|(line, (_, new))| match line {
                    HunkLine::Delete(_) => Some(new.saturating_sub(1)),
                    _ => None,
                })
                .unwrap_or(0)
        });
    format!(
        "@@ -{} +{} @@\n",
        format_hunk_range(old_start, old_count),
        format_hunk_range(new_start, new_count)
    )
}

fn split_islands(file_header: &str, body: &str) -> Option<Vec<(String, String)>> {
    let mut lines = body.lines();
    let header_line = lines.next()?;
    let (old_start, _, new_start, _) = parse_hunk_header(header_line)?;
    let lines: Vec<HunkLine> = lines.map(parse_hunk_line).collect();
    let islands = change_islands(&lines);
    if islands.is_empty() {
        return None;
    }
    let numbers = number_hunk_lines(old_start, new_start, &lines);
    Some(
        islands
            .into_iter()
            .map(|(from, to)| {
                let (start, end) = island_span(&lines, from, to);
                let island_lines = &lines[start..=end];
                let island_numbers = &numbers[start..=end];
                let hunk_header = island_header(island_lines, island_numbers);
                let complete = format!(
                    "{file_header}{hunk_header}{}",
                    write_hunk_lines(island_lines)
                );
                (hunk_header.trim_end().to_string(), complete)
            })
            .collect(),
    )
}

pub(super) fn split_hunks(
    snapshot_id: u64,
    path: &str,
    staged: bool,
    patch: &str,
) -> Vec<DiffHunk> {
    let Some(first) = patch.find("@@") else {
        return Vec::new();
    };
    let header = &patch[..first];
    let mut starts: Vec<usize> = patch
        .match_indices("@@")
        .filter(|(index, _)| *index == first || patch[..*index].ends_with('\n'))
        .map(|(index, _)| index)
        .collect();
    starts.push(patch.len());
    let mut hunks = Vec::new();
    for range in starts.windows(2) {
        let body = &patch[range[0]..range[1]];
        match split_islands(header, body) {
            Some(islands) if !islands.is_empty() => hunks.extend(islands),
            _ => hunks.push((
                body.lines().next().unwrap_or("@@").to_string(),
                format!("{header}{body}"),
            )),
        }
    }
    hunks
        .into_iter()
        .enumerate()
        .map(|(index, (hunk_header, complete))| {
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

fn untracked_line_stats(cwd: &Path, path: &str) -> (Option<u32>, Option<u32>) {
    let bytes = match fs::read(cwd.join(path)) {
        Ok(bytes) if bytes.len() <= MAX_DIFF_BYTES => bytes,
        _ => return (None, None),
    };
    if bytes.contains(&0) {
        return (None, None);
    }
    let newlines = bytes.iter().filter(|byte| **byte == b'\n').count() as u32;
    let additions = if bytes.is_empty() {
        0
    } else if bytes.ends_with(b"\n") {
        newlines
    } else {
        newlines + 1
    };
    (Some(additions), Some(0))
}
