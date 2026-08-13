import { invoke } from "@tauri-apps/api/core";

export type RepositoryKind = "workTree" | "bare" | "missing";
export type Language = "en" | "zh-CN";
export type RiskLevel = "normal" | "caution" | "destructive";

export interface GitInfo { path?: string; version?: string; supported: boolean; error?: string }
export interface RepositoryCapabilities { canRead: boolean; canWriteWorkTree: boolean; canManageRefs: boolean; canManageRemotes: boolean }
export interface OngoingGitState { kind: "merge" | "rebase" | "cherryPick" | "revert"; canContinue: boolean; canSkip: boolean; canAbort: boolean }
export interface RepositorySummary {
  id: number; path: string; name: string; group?: string; favorite: boolean; order: number; kind: RepositoryKind;
  capabilities: RepositoryCapabilities; branch?: string; headOid?: string; changedCount: number; conflictCount: number;
  ahead: number; behind: number; lastCommit?: string; ongoing?: OngoingGitState; error?: string;
}
export interface RepositoryRefresh { summary: RepositorySummary; snapshot?: WorkingTreeSnapshot }
export interface RepositoryRecord { id: number; path: string; name: string; group?: string; favorite: boolean; order: number }
export interface RepositoryPlacement { id: number; group?: string; favorite: boolean; order: number }
export interface Bootstrap { git: GitInfo; settings: { selectedRepositoryId?: number; leftWidth: number; rightWidth: number; outputHeight: number; language?: Language; groupOrder?: string[] }; repositories: RepositorySummary[] }
export interface FileChange {
  path: string; originalPath?: string; kind: string; indexStatus?: string; worktreeStatus?: string;
  staged: boolean; unstaged: boolean; conflict: boolean; ignored: boolean;
}
export interface WorkingTreeSnapshot { id: number; repositoryId: number; headOid?: string; files: FileChange[] }
export interface DiffHunk { id: string; header: string; patch: string }
export interface DiffFile { path: string; staged: boolean; binary: boolean; tooLarge: boolean; patch: string; hunks: DiffHunk[] }
export type ConflictSegment = { type: "context"; text: string } | { type: "conflict"; id: string; base: string; current: string; incoming: string };
export interface ConflictDocument { id: string; path: string; segments: ConflictSegment[] }
export type ConflictChoice = "current" | "incoming" | "both";
export interface ConflictResolution { blockId: string; choice: ConflictChoice }
export interface CommitInfo { oid: string; parents: string[]; author: string; authoredAt: string; subject: string; refs: string[]; lane: { column: number; parentColumns: number[] } }
export interface HistoryCursor { offset: number; activeLanes: string[] }
export interface CommitPage { commits: CommitInfo[]; nextCursor?: HistoryCursor | null }
export interface BranchInfo { name: string; oid: string; current: boolean; remote: boolean; upstream?: string }
export interface TagInfo { name: string; oid: string; subject: string }
export interface RemoteInfo { name: string; fetchUrl: string; pushUrl: string }
export interface StashInfo { index: number; oid: string; subject: string }
export interface SubmoduleInfo { path: string; oid: string; initialized: boolean; state: string }
export interface OperationPreview { title: string; summary: string; risk: RiskLevel; affectedPaths: string[]; affectedRefs: string[]; recoverable: boolean; requiresConfirmation: boolean }
export interface OperationEvent { operationId: number; repositoryId?: number | null; kind: "started" | "stdout" | "stderr" | "finished"; message: string; exitCode?: number; outcome?: "succeeded" | "failed" | "cancelled" }
export interface OperationResult { operationId: number; accepted: boolean }
export interface SessionLogLine { timestamp: string; kind: OperationEvent["kind"] | "error"; message: string }
export type OperationRequest = { type: string; [key: string]: unknown };

export const api = {
  bootstrap: () => invoke<Bootstrap>("bootstrap"),
  refreshRepositories: (activeRepositoryId?: number) => invoke<RepositorySummary[]>("refresh_repositories", { activeRepositoryId }),
  refreshRepository: (repositoryId: number) => invoke<RepositoryRefresh>("refresh_repository", { repositoryId }),
  addRepository: (path: string) => invoke<RepositorySummary>("add_repository", { path }),
  initRepository: (path: string) => invoke<RepositorySummary>("initialize_repository", { path }),
  cloneRepository: (url: string, destination: string) => invoke<OperationResult>("clone_repository", { url, destination }),
  removeRepository: (repositoryId: number) => invoke<void>("remove_repository", { repositoryId }),
  relocateRepository: (repositoryId: number, path: string) => invoke<RepositorySummary>("relocate_repository", { repositoryId, path }),
  updateRepository: (repository: RepositoryRecord) => invoke<void>("update_repository", { repository }),
  reorderRepositories: (placements: RepositoryPlacement[]) => invoke<void>("reorder_repositories", { placements }),
  saveGroupOrder: (groups: string[]) => invoke<void>("save_group_order", { groups }),
  setGitPath: (path?: string) => invoke<GitInfo>("set_git_path", { path }),
  saveLayout: (leftWidth: number, rightWidth: number, outputHeight: number) => invoke<void>("save_layout", { leftWidth, rightWidth, outputHeight }),
  saveLanguage: (language: Language) => invoke<void>("save_language", { language }),
  watchRepository: (repositoryId: number) => invoke<void>("watch_repository", { repositoryId }),
  status: (repositoryId: number, includeIgnored = false) => invoke<WorkingTreeSnapshot>("get_status", { repositoryId, includeIgnored }),
  diff: (repositoryId: number, snapshotId: number, path: string, staged: boolean) => invoke<DiffFile>("get_diff", { repositoryId, snapshotId, path, staged }),
  conflictDocument: (repositoryId: number, snapshotId: number, path: string) => invoke<ConflictDocument>("get_conflict_document", { repositoryId, snapshotId, path }),
  history: (repositoryId: number, cursor?: HistoryCursor, limit = 100) => invoke<CommitPage>("get_history", { repositoryId, cursor, limit }),
  exportSessionLog: (fileName: string, lines: SessionLogLine[]) => invoke<boolean>("export_session_log", { fileName, lines }),
  commitDiff: (repositoryId: number, oid: string) => invoke<string>("get_commit_diff", { repositoryId, oid }),
  compareBranches: (repositoryId: number, base: string, head: string) => invoke<string>("compare_branches", { repositoryId, base, head }),
  openRepositoryFile: (repositoryId: number, path: string) => invoke<void>("open_repository_file", { repositoryId, path }),
  branches: (repositoryId: number) => invoke<BranchInfo[]>("get_branches", { repositoryId }),
  tags: (repositoryId: number) => invoke<TagInfo[]>("get_tags", { repositoryId }),
  remotes: (repositoryId: number) => invoke<RemoteInfo[]>("get_remotes", { repositoryId }),
  stashes: (repositoryId: number) => invoke<StashInfo[]>("get_stashes", { repositoryId }),
  submodules: (repositoryId: number) => invoke<SubmoduleInfo[]>("get_submodules", { repositoryId }),
  preview: (repositoryId: number, request: OperationRequest) => invoke<OperationPreview>("preview_operation", { repositoryId, request }),
  start: (repositoryId: number, request: OperationRequest, confirmed = false) => invoke<OperationResult>("start_operation", { repositoryId, request, confirmed }),
  cancel: (operationId: number) => invoke<void>("cancel_operation", { operationId }),
};
