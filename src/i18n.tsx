import { createContext, useContext, useMemo, type ReactNode } from "react";

export type Language = "en" | "zh-CN";

const en = {
  gitUnavailable: "Git unavailable", searchRepositories: "Search repositories", findRepository: "Find repository…", repositories: "Repositories",
  add: "Add", clone: "Clone", initialize: "Initialize", changes: "Changes", history: "History", branches: "Branches", stashes: "Stashes", workflows: "Workflows",
  fetch: "Fetch", pull: "Pull", push: "Push", more: "More", moreActions: "More actions", batchActions: "Batch actions", refreshAll: "Refresh all", pullMerge: "Pull with merge",
  pullRebase: "Pull with rebase", pullFf: "Pull fast-forward only", forcePush: "Force push with lease", setUpstream: "Set upstream",
  cherryPickCommits: "Cherry-pick commits", undoCommit: "Undo last commit", renameEntry: "Rename entry", removeFavorite: "Remove favorite",
  addFavorite: "Add favorite", addGroup: "Add group", setGroup: "Set group", relocate: "Relocate", selectGit: "Select Git executable", removeGitDock: "Remove from GitDock",
  remoteUrl: "Remote URL", remote: "Remote", commitOids: "Commit OIDs, separated by spaces", repositoryName: "Repository name", group: "Group",
  gitOutput: "Git output", running: "running", lines: "lines", cancel: "Cancel", confirm: "Confirm", clear: "Clear", exportLog: "Export log", language: "中文",
  favorites: "Favorites", ungrouped: "Ungrouped", renameGroup: "Rename group", ungroup: "Remove group", moveUp: "Move up", moveDown: "Move down",
  emptyTitle1: "Put every working tree", emptyTitle2: "on one rail.", emptyDescription: "Inspect changes, shape commits, and move between branches without losing the state of another repository.",
  addRepository: "Add repository", gitRequired: "Git 2.30+ required", conflicts: "conflicts", missing: "missing", conflict: "conflict", changed: "changed", clean: "clean",
  workingTreeClean: "Working tree clean", noLocalChanges: "No local changes", workingTree: "Working tree", workingTreeChanges: "working tree changes",
  selectRepository: "Select a repository", selectRepositoryHint: "Choose a working tree from the left sidebar to inspect changes.",
  selectFile: "Select a file to inspect", inspectHint: "The center canvas shows the exact patch GitDock will stage or unstage.",
  conflictsGroup: "Conflicts", staged: "Staged", mixed: "Partially staged", unstaged: "Unstaged", untracked: "Untracked", ignored: "Ignored", inProgress: "in progress",
  continue: "Continue", skip: "Skip", abort: "Abort", commitMessage: "Commit message", commitPlaceholder: "Summarize the change…", amend: "Amend",
  signOff: "Sign off", commitStaged: "Commit staged changes", stage: "Stage", unstage: "Unstage", stageSelected: "Stage selected", unstageSelected: "Unstage selected",
  selectAll: "Select all", selectFileForStage: "Select file to stage", selectFileForUnstage: "Select file to unstage", selectFileForStageOrUnstage: "Select file to stage or unstage", trash: "Trash", discard: "Discard",
  resolve: "Resolve", useCurrent: "Use current target", useIncoming: "Use incoming commit", openExternal: "Open externally", runMergetool: "Run configured mergetool",
  markResolved: "Mark resolved", openInternalEditor: "Open internal editor", back: "Back", binaryDiff: "Binary diff", diffTooLarge: "Diff exceeds the safe preview limit", openDifftool: "Open configured difftool",
  baseVersion: "Base", currentVersion: "Current", incomingVersion: "Incoming", conflictBlock: "Conflict block", useBoth: "Use both", resolveAndStage: "Resolve and stage", blocksRemaining: "blocks remaining", readyToResolve: "Ready to resolve",
  commandPalette: "Command palette", searchCommands: "Search commands…", noMatchingCommands: "No matching commands",
  loading: "Loading…", emptyBrand: "GitDock / Workspace", favorite: "Favorite", noMatchingRepositories: "No matching repositories",
  searchBranches: "Search branches", findBranch: "Find branch…", noMatchingBranches: "No matching branches",
  noStashes: "No stashes yet", noTags: "No tags", noRemotes: "No remotes", noSubmodules: "No submodules", noBranches: "No branches",
  unstageHunk: "Unstage hunk", stageHunk: "Stage hunk", discardHunk: "Discard hunk", repositoryGraph: "Repository graph", commitsLoaded: "commits loaded", commits: "Commits", loadMore: "Load more",
  commitDiff: "Commit", branchComparison: "Branch comparison", changedFiles: "Changed files", binaryFile: "binary", commitId: "Commit ID", date: "Date",
  cherryPick: "Cherry-pick", revert: "Revert", detachedHead: "detached HEAD", refsIntegration: "Refs and integration",
  branchHint: "Choose a branch, tag, remote, or submodule from the right pane.", savedStates: "saved working tree states", stashHint: "Apply, pop, or inspect a saved worktree state.",
  tags: "Tags", remotes: "Remotes", submodules: "Submodules", localBranches: "Local branches", remoteBranches: "Remote branches", compare: "Compare", newBranch: "New branch", newBranchName: "New branch name",
  create: "Create", deleteRemoteBranch: "Delete remote branch", switch: "Switch", merge: "Merge", fastForward: "Fast-forward only", squashMerge: "Squash merge",
  rebaseOnto: "Rebase onto", rename: "Rename", delete: "Delete", forceDelete: "Force delete", baseBranch: "Base branch", checkoutBranch: "Checkout after creating",
  tagName: "Tag name", annotation: "Annotation (leave empty for lightweight tag)", pushTag: "Push tag", deleteLocalTag: "Delete local tag",
  remoteName: "Remote name", newRemoteUrl: "New remote URL", editUrl: "Edit URL", removeRemote: "Remove remote", init: "Init", sync: "Sync", update: "Update",
  updateNested: "Update nested submodules recursively?", stashMessage: "Stash message (optional)", includeUntracked: "Include untracked files?", apply: "Apply", pop: "Pop", drop: "Drop",
  irreversible: "Irreversible change", reviewOperation: "Review operation", affectedPaths: "Affected paths", affectedRefs: "Affected refs",
  recoverable: "Git can usually recover this change.", unrecoverable: "GitDock cannot recover this change.", closeOperations: "Git operation(s) are still running. Cancel them and quit?",
  removeRepositoryConfirm: "from GitDock? Files on disk are not changed.", fetchBeforeForce: "before force pushing.", operationSucceeded: "Succeeded",
  operationFailed: "Failed", operationCancelled: "Cancelled", dismissNotification: "Dismiss notification",
  interactiveRebase: "Interactive rebase", startRebase: "Start rebase", onto: "Rebase onto", pick: "Pick", reword: "Reword", squash: "Squash", fixup: "Fixup",
  rewordMessage: "New message", noCommitsToRebase: "No commits to rebase onto this base", fileHistory: "File history", blame: "Blame", historyOf: "History of", blameOf: "Blame of", author: "Author", noHistory: "No history for this file",
  resizeRepositories: "Resize repository sidebar", resizeDetails: "Resize details pane", resizeOutput: "Resize Git output", moveCommitUp: "Move commit up", moveCommitDown: "Move commit down", rebaseAction: "Rebase action",
} as const;

export type MessageKey = keyof typeof en;

const zh: Record<MessageKey, string> = {
  gitUnavailable: "Git 不可用", searchRepositories: "搜索仓库", findRepository: "查找仓库…", repositories: "仓库",
  add: "添加", clone: "克隆", initialize: "初始化", changes: "更改", history: "历史", branches: "分支", stashes: "贮藏", workflows: "工作流",
  fetch: "获取", pull: "拉取", push: "推送", more: "更多", moreActions: "更多操作", batchActions: "批量操作", refreshAll: "刷新全部", pullMerge: "合并式拉取",
  pullRebase: "变基式拉取", pullFf: "仅快进拉取", forcePush: "带租约强制推送", setUpstream: "设置上游",
  cherryPickCommits: "拣选提交", undoCommit: "撤销上次提交", renameEntry: "重命名条目", removeFavorite: "取消收藏",
  addFavorite: "添加收藏", addGroup: "添加分组", setGroup: "设置分组", relocate: "重新定位", selectGit: "选择 Git 可执行文件", removeGitDock: "从 GitDock 移除",
  remoteUrl: "远程 URL", remote: "远程仓库", commitOids: "提交 OID，以空格分隔", repositoryName: "仓库名称", group: "分组",
  gitOutput: "Git 输出", running: "运行中", lines: "行", cancel: "取消", confirm: "确认", clear: "清空", exportLog: "导出日志", language: "EN",
  favorites: "收藏", ungrouped: "未分组", renameGroup: "重命名分组", ungroup: "取消分组", moveUp: "上移", moveDown: "下移",
  emptyTitle1: "让每个工作区", emptyTitle2: "各就其位。", emptyDescription: "检查更改、组织提交并切换分支，同时保留其他仓库的状态。",
  addRepository: "添加仓库", gitRequired: "需要 Git 2.30+", conflicts: "个冲突", missing: "缺失", conflict: "冲突", changed: "有更改", clean: "干净",
  workingTreeClean: "工作区干净", noLocalChanges: "没有本地更改", workingTree: "工作区", workingTreeChanges: "项工作区更改",
  selectRepository: "选择一个仓库", selectRepositoryHint: "从左侧栏选择一个工作区以检查更改。",
  selectFile: "选择文件以检查", inspectHint: "中央区域会显示 GitDock 将暂存或取消暂存的准确补丁。",
  conflictsGroup: "冲突", staged: "已暂存", mixed: "部分暂存", unstaged: "未暂存", untracked: "未跟踪", ignored: "已忽略", inProgress: "进行中",
  continue: "继续", skip: "跳过", abort: "中止", commitMessage: "提交信息", commitPlaceholder: "概述本次更改…", amend: "修订提交",
  signOff: "签署", commitStaged: "提交已暂存更改", stage: "暂存", unstage: "取消暂存", stageSelected: "暂存所选", unstageSelected: "取消暂存所选",
  selectAll: "全选", selectFileForStage: "选择要暂存的文件", selectFileForUnstage: "选择要取消暂存的文件", selectFileForStageOrUnstage: "选择要暂存或取消暂存的文件", trash: "删除", discard: "丢弃",
  resolve: "解决", useCurrent: "使用当前目标", useIncoming: "使用传入提交", openExternal: "在外部打开", runMergetool: "运行已配置的合并工具",
  markResolved: "标记为已解决", openInternalEditor: "打开内部编辑器", back: "返回", binaryDiff: "二进制差异", diffTooLarge: "差异超过安全预览限制", openDifftool: "打开已配置的差异工具",
  baseVersion: "基础版本", currentVersion: "当前版本", incomingVersion: "传入版本", conflictBlock: "冲突块", useBoth: "保留两者", resolveAndStage: "解决并暂存", blocksRemaining: "个冲突块未选择", readyToResolve: "可以解决冲突",
  commandPalette: "命令面板", searchCommands: "搜索命令…", noMatchingCommands: "没有匹配的命令",
  loading: "加载中…", emptyBrand: "GitDock / 工作区", favorite: "收藏", noMatchingRepositories: "没有匹配的仓库",
  searchBranches: "搜索分支", findBranch: "查找分支…", noMatchingBranches: "没有匹配的分支",
  noStashes: "还没有贮藏", noTags: "没有标签", noRemotes: "没有远程仓库", noSubmodules: "没有子模块", noBranches: "没有分支",
  unstageHunk: "取消暂存区块", stageHunk: "暂存区块", discardHunk: "丢弃区块", repositoryGraph: "仓库图", commitsLoaded: "个提交已加载", commits: "提交", loadMore: "加载更多",
  commitDiff: "提交", branchComparison: "分支比较", changedFiles: "变更文件", binaryFile: "二进制", commitId: "提交 ID", date: "日期",
  cherryPick: "拣选", revert: "还原", detachedHead: "分离的 HEAD", refsIntegration: "引用与集成",
  branchHint: "从右侧面板选择分支、标签、远程仓库或子模块。", savedStates: "保存的工作区状态", stashHint: "应用、弹出或检查保存的工作区状态。",
  tags: "标签", remotes: "远程", submodules: "子模块", localBranches: "本地分支", remoteBranches: "远程分支", compare: "比较", newBranch: "新建分支", newBranchName: "新分支名称",
  create: "创建", deleteRemoteBranch: "删除远程分支", switch: "切换", merge: "合并", fastForward: "仅快进", squashMerge: "压缩合并",
  rebaseOnto: "变基到此处", rename: "重命名", delete: "删除", forceDelete: "强制删除", baseBranch: "基础分支", checkoutBranch: "创建后切换",
  tagName: "标签名称", annotation: "注释（留空则创建轻量标签）", pushTag: "推送标签", deleteLocalTag: "删除本地标签",
  remoteName: "远程名称", newRemoteUrl: "新远程 URL", editUrl: "编辑 URL", removeRemote: "移除远程", init: "初始化", sync: "同步", update: "更新",
  updateNested: "递归更新嵌套子模块？", stashMessage: "贮藏信息（可选）", includeUntracked: "包含未跟踪文件？", apply: "应用", pop: "弹出", drop: "删除",
  irreversible: "不可逆更改", reviewOperation: "检查操作", affectedPaths: "受影响路径", affectedRefs: "受影响引用",
  recoverable: "Git 通常可以恢复此更改。", unrecoverable: "GitDock 无法恢复此更改。", closeOperations: "个 Git 操作仍在运行。取消并退出？",
  removeRepositoryConfirm: "从 GitDock 移除？磁盘文件不会改变。", fetchBeforeForce: "后再强制推送。", operationSucceeded: "操作成功",
  operationFailed: "操作失败", operationCancelled: "操作已取消", dismissNotification: "关闭通知",
  interactiveRebase: "交互式变基", startRebase: "开始变基", onto: "变基目标", pick: "保留", reword: "改写提交信息", squash: "压缩", fixup: "修复",
  rewordMessage: "新的提交信息", noCommitsToRebase: "该基础上没有可变基的提交", fileHistory: "文件历史", blame: "追溯", historyOf: "历史：", blameOf: "追溯：", author: "作者", noHistory: "该文件没有历史记录",
  resizeRepositories: "调整仓库侧栏宽度", resizeDetails: "调整详情面板宽度", resizeOutput: "调整 Git 输出高度", moveCommitUp: "上移提交", moveCommitDown: "下移提交", rebaseAction: "变基操作",
};

const dictionaries: Record<Language, Record<MessageKey, string>> = { en, "zh-CN": zh };
export const translate = (language: Language, key: MessageKey) => dictionaries[language][key];
const I18nContext = createContext({ language: "en" as Language, t: (key: MessageKey) => en[key] as string });

export function I18nProvider({ language, children }: { language: Language; children: ReactNode }) {
  const value = useMemo(() => ({ language, t: (key: MessageKey) => dictionaries[language][key] }), [language]);
  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export const useI18n = () => useContext(I18nContext);
