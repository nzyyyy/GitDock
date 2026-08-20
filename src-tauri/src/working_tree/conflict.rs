use crate::{
    git::{ensure_success, error_text, strings, Git, MAX_DIFF_BYTES, MAX_DIFF_LINES},
    models::{ConflictChoice, ConflictDocument, ConflictResolution, ConflictSegment},
};
use std::{
    collections::{hash_map::DefaultHasher, HashMap, HashSet},
    fs,
    hash::{Hash, Hasher},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(super) struct ConflictStage {
    pub(super) mode: String,
    pub(super) oid: String,
}

#[derive(Debug, Clone)]
pub(super) struct ConflictSource {
    pub(super) document: ConflictDocument,
    pub(super) stages: [ConflictStage; 3],
    pub(super) worktree: Vec<u8>,
    pub(super) worktree_executable: bool,
}

pub(super) fn source(
    git: &Git,
    cwd: &Path,
    path: &str,
    snapshot_id: u64,
) -> Result<ConflictSource, String> {
    let stages = stages(git, cwd, path)?;
    let contents = stages
        .iter()
        .map(|stage| filtered_blob(git, cwd, path, &stage.oid))
        .collect::<Result<Vec<_>, _>>()?;
    let [base, current, incoming]: [Vec<u8>; 3] = contents
        .try_into()
        .map_err(|_| "A three-stage conflict is required".to_string())?;
    for content in [&base, &current, &incoming] {
        validate_text(content)?;
    }
    let worktree = fs::read(cwd.join(path)).map_err(|error| error.to_string())?;
    validate_text(&worktree)?;
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
    let mut incoming_file = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
    current_file
        .write_all(&current)
        .map_err(|error| error.to_string())?;
    base_file
        .write_all(&base)
        .map_err(|error| error.to_string())?;
    incoming_file
        .write_all(&incoming)
        .map_err(|error| error.to_string())?;
    let output = git.run(
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
    let segments = parse_segments(snapshot_id, path, &merged, &labels, originals)?;
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

pub(super) fn stages(git: &Git, cwd: &Path, path: &str) -> Result<[ConflictStage; 3], String> {
    let output = ensure_success(git.run(
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
        let record =
            std::str::from_utf8(record).map_err(|_| "Conflict index entry is not valid UTF-8")?;
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
            "Only regular files with matching modes can use the internal conflict editor".into(),
        );
    }
    Ok([base, current, incoming])
}

fn filtered_blob(git: &Git, cwd: &Path, path: &str, oid: &str) -> Result<Vec<u8>, String> {
    let output = ensure_success(git.run(
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

pub(super) fn resolve(
    git: &Git,
    root: &Path,
    path: &str,
    source: &ConflictSource,
    choices: &[ConflictResolution],
) -> Result<(), String> {
    let target = safe_worktree_file(root, path)?;
    if stages(git, root, path)? != source.stages {
        return Err(
            "Conflict stages changed after this editor was opened. Refresh and try again.".into(),
        );
    }
    if fs::read(&target).map_err(|error| error.to_string())? != source.worktree {
        return Err(
            "The working-tree file changed after this editor was opened. Refresh and try again."
                .into(),
        );
    }
    if file_executable(&target)? != source.worktree_executable {
        return Err("The working-tree file mode changed after this editor was opened. Refresh and try again.".into());
    }
    let result = render_resolution(&source.document.segments, choices)?;
    let permissions = fs::metadata(&target)
        .map_err(|error| error.to_string())?
        .permissions();
    replace_file(&target, &result, permissions.clone())?;
    let add = ensure_success(git.run(root, &strings(&["add", "--", path]), None)?);
    if let Err(error) = add {
        if let Err(restore_error) = replace_file(&target, &source.worktree, permissions) {
            return Err(format!(
                "{error}; restoring the original file also failed: {restore_error}"
            ));
        }
        return Err(error);
    }
    Ok(())
}

pub(super) fn safe_worktree_file(root: &Path, path: &str) -> Result<PathBuf, String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let target = root.join(path);
    let metadata = fs::symlink_metadata(&target).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_file() {
        return Err("Only regular files can use the internal conflict editor".into());
    }
    let parent = target
        .parent()
        .ok_or("Conflict path has no parent")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !parent.starts_with(&root) {
        return Err("Conflict path resolves outside the repository".into());
    }
    Ok(target)
}

fn replace_file(
    target: &Path,
    contents: &[u8],
    permissions: fs::Permissions,
) -> Result<(), String> {
    let parent = target.parent().ok_or("Conflict path has no parent")?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    temporary
        .write_all(contents)
        .map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .set_permissions(permissions)
        .map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    temporary
        .persist(target)
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) fn render_resolution(
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

fn file_executable(path: &Path) -> Result<bool, String> {
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

pub(super) fn validate_text(content: &[u8]) -> Result<(), String> {
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

pub(super) fn parse_segments(
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
