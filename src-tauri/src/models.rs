use serde::{Deserialize, Serialize};
use specta::Type;

pub type RepositoryId = u64;
pub type OperationId = u64;

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitInfo {
    pub path: Option<String>,
    pub version: Option<String>,
    pub supported: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub git_path: Option<String>,
    pub selected_repository_id: Option<RepositoryId>,
    pub left_width: u16,
    pub right_width: u16,
    pub output_height: u16,
    #[serde(default)]
    pub language: Language,
    #[serde(default)]
    pub group_order: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Type, PartialEq, Eq)]
pub enum Language {
    #[default]
    #[serde(rename = "en")]
    English,
    #[serde(rename = "zh-CN")]
    SimplifiedChinese,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            git_path: None,
            selected_repository_id: None,
            left_width: 240,
            right_width: 360,
            output_height: 190,
            language: Language::English,
            group_order: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryRecord {
    pub id: RepositoryId,
    pub path: String,
    pub name: String,
    pub group: Option<String>,
    pub favorite: bool,
    pub order: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RepositoryKind {
    WorkTree,
    Bare,
    Missing,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryCapabilities {
    pub can_read: bool,
    pub can_write_work_tree: bool,
    pub can_manage_refs: bool,
    pub can_manage_remotes: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RepositorySummary {
    pub id: RepositoryId,
    pub path: String,
    pub name: String,
    pub group: Option<String>,
    pub favorite: bool,
    pub order: u32,
    pub kind: RepositoryKind,
    pub capabilities: RepositoryCapabilities,
    pub branch: Option<String>,
    pub head_oid: Option<String>,
    pub changed_count: usize,
    pub conflict_count: usize,
    pub ahead: u32,
    pub behind: u32,
    pub last_commit: Option<String>,
    pub ongoing: Option<OngoingGitState>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryRefresh {
    pub summary: RepositorySummary,
    pub snapshot: Option<WorkingTreeSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryPlacement {
    pub id: RepositoryId,
    pub group: Option<String>,
    pub favorite: bool,
    pub order: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Bootstrap {
    pub git: GitInfo,
    pub settings: Settings,
    pub repositories: Vec<RepositorySummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    Untracked,
    Ignored,
    Conflicted,
    TypeChanged,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    pub original_path: Option<String>,
    pub kind: ChangeKind,
    pub index_status: Option<String>,
    pub worktree_status: Option<String>,
    pub staged: bool,
    pub unstaged: bool,
    pub conflict: bool,
    pub ignored: bool,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkingTreeSnapshot {
    pub id: u64,
    pub repository_id: RepositoryId,
    pub head_oid: Option<String>,
    pub files: Vec<FileChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunk {
    pub id: String,
    pub header: String,
    pub patch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiffFile {
    pub path: String,
    pub staged: bool,
    pub binary: bool,
    pub too_large: bool,
    pub patch: String,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConflictDocument {
    pub id: String,
    pub path: String,
    pub segments: Vec<ConflictSegment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ConflictSegment {
    Context {
        text: String,
    },
    Conflict {
        id: String,
        base: String,
        current: String,
        incoming: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConflictResolution {
    pub block_id: String,
    pub choice: ConflictChoice,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ConflictChoice {
    Current,
    Incoming,
    Both,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphLane {
    pub column: usize,
    pub parent_columns: Vec<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommitInfo {
    pub oid: String,
    pub parents: Vec<String>,
    pub author: String,
    pub authored_at: String,
    pub subject: String,
    pub refs: Vec<String>,
    pub lane: GraphLane,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryCursor {
    pub offset: usize,
    pub active_lanes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommitPage {
    pub commits: Vec<CommitInfo>,
    pub next_cursor: Option<HistoryCursor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommitFileChange {
    pub path: String,
    pub original_path: Option<String>,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommitDetail {
    pub oid: String,
    pub author: String,
    pub email: String,
    pub authored_at: String,
    pub message: String,
    pub files: Vec<CommitFileChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionLogLine {
    pub timestamp: String,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    pub name: String,
    pub oid: String,
    pub current: bool,
    pub remote: bool,
    pub upstream: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TagInfo {
    pub name: String,
    pub oid: String,
    pub subject: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteInfo {
    pub name: String,
    pub fetch_url: String,
    pub push_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StashInfo {
    pub index: usize,
    pub oid: String,
    pub subject: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubmoduleInfo {
    pub path: String,
    pub oid: String,
    pub initialized: bool,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryEntry {
    pub oid: String,
    pub author: String,
    pub authored_at: String,
    pub subject: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BlameHunk {
    pub oid: String,
    pub author: String,
    pub author_time: i64,
    pub start_line: usize,
    pub line_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BlameFile {
    pub path: String,
    pub content: Vec<String>,
    pub hunks: Vec<BlameHunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RebaseCommit {
    pub oid: String,
    pub subject: String,
    pub author: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RebaseAction {
    Pick,
    Reword,
    Squash,
    Fixup,
    Drop,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RebaseStep {
    pub oid: String,
    pub action: RebaseAction,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OngoingGitState {
    pub kind: OngoingKind,
    pub can_continue: bool,
    pub can_skip: bool,
    pub can_abort: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OngoingKind {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum RiskLevel {
    Normal,
    Caution,
    Destructive,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OperationPreview {
    pub title: String,
    pub summary: String,
    pub risk: RiskLevel,
    pub affected_paths: Vec<String>,
    pub affected_refs: Vec<String>,
    pub recoverable: bool,
    pub requires_confirmation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OperationRequest {
    StageFiles {
        paths: Vec<String>,
    },
    UnstageFiles {
        paths: Vec<String>,
    },
    StageHunk {
        snapshot_id: u64,
        hunk_id: String,
    },
    UnstageHunk {
        snapshot_id: u64,
        hunk_id: String,
    },
    DiscardHunk {
        snapshot_id: u64,
        hunk_id: String,
    },
    DiscardTracked {
        paths: Vec<String>,
    },
    TrashUntracked {
        paths: Vec<String>,
    },
    Commit {
        message: String,
        amend: bool,
        signoff: bool,
    },
    Fetch {
        remote: Option<String>,
        prune: bool,
    },
    Pull {
        strategy: Option<PullStrategy>,
    },
    Push {
        remote: Option<String>,
        branch: Option<String>,
    },
    ForcePushWithLease {
        remote: String,
        branch: String,
        expected_oid: String,
    },
    SetUpstream {
        remote: String,
        branch: String,
    },
    AddRemote {
        name: String,
        url: String,
    },
    SetRemoteUrl {
        name: String,
        url: String,
    },
    RemoveRemote {
        name: String,
    },
    DeleteRemoteBranch {
        remote: String,
        branch: String,
    },
    CreateBranch {
        name: String,
        start_point: Option<String>,
        checkout: bool,
    },
    SwitchBranch {
        name: String,
    },
    RenameBranch {
        old_name: String,
        new_name: String,
    },
    DeleteBranch {
        name: String,
        force: bool,
    },
    Merge {
        reference: String,
        mode: MergeMode,
    },
    Rebase {
        onto: String,
    },
    InteractiveRebase {
        onto: String,
        plan: Vec<RebaseStep>,
    },
    CherryPick {
        commits: Vec<String>,
    },
    Continue {
        kind: OngoingKind,
    },
    Skip {
        kind: OngoingKind,
    },
    Abort {
        kind: OngoingKind,
    },
    ChooseConflictSide {
        path: String,
        side: ConflictSide,
    },
    MarkResolved {
        paths: Vec<String>,
    },
    ResolveConflictBlocks {
        snapshot_id: u64,
        document_id: String,
        path: String,
        choices: Vec<ConflictResolution>,
    },
    StashCreate {
        message: Option<String>,
        include_untracked: bool,
    },
    StashApply {
        index: usize,
        pop: bool,
    },
    StashDrop {
        index: usize,
    },
    CreateTag {
        name: String,
        target: Option<String>,
        message: Option<String>,
    },
    DeleteLocalTag {
        name: String,
    },
    PushTag {
        remote: String,
        name: String,
    },
    SubmoduleInit {
        paths: Vec<String>,
        recursive: bool,
    },
    SubmoduleUpdate {
        paths: Vec<String>,
        recursive: bool,
    },
    SubmoduleSync {
        paths: Vec<String>,
        recursive: bool,
    },
    Revert {
        oid: String,
    },
    UndoLastCommit,
    RunDifftool {
        path: String,
        staged: bool,
    },
    RunMergetool {
        path: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PullStrategy {
    Merge,
    Rebase,
    FastForwardOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MergeMode {
    FastForward,
    Normal,
    Squash,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ConflictSide {
    Ours,
    Theirs,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OperationEventKind {
    Started,
    Stdout,
    Stderr,
    Finished,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OperationOutcome {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OperationEvent {
    pub operation_id: OperationId,
    pub repository_id: Option<RepositoryId>,
    pub kind: OperationEventKind,
    pub message: String,
    pub exit_code: Option<i32>,
    pub outcome: Option<OperationOutcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OperationResult {
    pub operation_id: OperationId,
    pub accepted: bool,
}
