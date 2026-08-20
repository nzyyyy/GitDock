import { memo, useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, type ConflictResolution, type GitInfo, type Language, type RepositorySummary } from "./api";
import { ConflictEditor } from "./ConflictEditor";
import { DiffView } from "./DiffView";
import { I18nProvider, translate } from "./i18n";
import { readLogs } from "./lib/logBuffer";
import { BranchCanvas, MemoBranchesPane } from "./components/BranchesPane";
import { MemoChangesOverview, MemoChangesPane } from "./components/ChangesPane";
import { BlameView } from "./components/BlameView";
import { CommitDetailView } from "./components/CommitDetailView";
import { FileHistoryView } from "./components/FileHistoryView";
import { InteractiveRebase } from "./components/InteractiveRebase";
import { CommandPalette } from "./components/CommandPalette";
import { MemoHistoryCanvas, MemoHistoryPane } from "./components/HistoryPane";
import { EmptyState, MemoRepositoryRow, RailMark, RowMenu } from "./components/RepositoryPane";
import { MemoStashesPane, StashCanvas } from "./components/StashesPane";
import { ConfirmDialog, FormDialog } from "./components/dialogs";
import { ToastStack } from "./components/toast";
import { useHistory } from "./hooks/useHistory";
import { useFileInspection } from "./hooks/useFileInspection";
import { useLogBuffer } from "./hooks/useLogBuffer";
import { useOperations } from "./hooks/useOperations";
import { useRepositoryList } from "./hooks/useRepositoryList";
import { useWorkingTree } from "./hooks/useWorkingTree";
import { errorMessage, FAVORITES_GROUP, shortOid, UNGROUPED_GROUP, type CommandItem, type DialogSpec, type Tab } from "./types";

const MemoDiffView = memo(DiffView);
const WORKFLOW_TABS: Tab[] = ["changes", "history", "branches", "stashes"];
type LayoutSide = "left" | "right" | "output";
const LAYOUT_LIMITS: Record<LayoutSide, readonly [number, number]> = { left: [190, 420], right: [300, 560], output: [120, 420] };
const clampLayout = (side: LayoutSide, value: number) => Math.max(LAYOUT_LIMITS[side][0], Math.min(LAYOUT_LIMITS[side][1], value));

export default function App() {
  const [git, setGit] = useState<GitInfo>({ path: null, version: null, supported: false, error: null });
  const [selectedId, setSelectedId] = useState<number>();
  const [tab, setTab] = useState<Tab>("changes");
  const [outputOpen, setOutputOpen] = useState(false);
  const [dialog, setDialog] = useState<DialogSpec>();
  const [leftWidth, setLeftWidth] = useState(240);
  const [rightWidth, setRightWidth] = useState(360);
  const [outputHeight, setOutputHeight] = useState(190);
  const [language, setLanguage] = useState<Language>("en");
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [rebaseDialog, setRebaseDialog] = useState<{ repositoryId: number; onto?: string }>();
  const selectedIdRef = useRef<number | undefined>(undefined);
  selectedIdRef.current = selectedId;
  const t = (key: Parameters<typeof translate>[1]) => translate(language, key);
  const showDialog = useCallback((spec: DialogSpec) => setDialog(spec), []);

  const { pushLog, exportLogs, clearLogs, logCount, lastLog, logBuffer } = useLogBuffer({ setOutputOpen });
  const reportError = useCallback((message: string) => { pushLog("error", message); setOutputOpen(true); }, [pushLog]);

  const history = useHistory({ reportError, selectedIdRef, selectedId, tab });
  const workingTree = useWorkingTree({ reportError, selectedIdRef, historyRepositoryRef: history.historyRepositoryRef, selectedId, language });
  const list = useRepositoryList({ reportError, selectedIdRef, setSnapshot: workingTree.setSnapshot, refreshStatus: workingTree.refreshStatus, t, language });
  const operations = useOperations({ pushLog, reportError, t, showDialog, setSelectedId, setOutputOpen, refreshRepositories: list.refreshRepositories, refreshHistory: history.refreshHistory, selectedId, selectedIdRef, historyRepositoryRef: history.historyRepositoryRef });
  const fileInspection = useFileInspection({ reportError, selectedId, selectedIdRef });

  const { repositories, setRepositories, setCustomGroups, filter, setFilter, collapsedGroups, setCollapsedGroups, draggingRepositoryId, dropTargetGroup, refreshRepositories, repositoryGroups, moveRepository, moveRepositoryBy, addGroup, updateGroup, acceptRepositoryDrop, hintRepositoryDrop, clearRepositoryDropHint } = list;
  const { snapshot, setSnapshot, diff, conflict, setConflict, selectedCommit, commitDetail, diffIsFile, statusRequest, refreshStatus, reloadOpenDiff, openDiff, closeDiff, closeCommitFile, loadIgnored, openCommit, openCommitFile, showBranchDiff } = workingTree;
  const { view: fileView, path: filePath, entries: fileHistoryEntries, selectedOid: fileHistoryOid, diff: fileDiff, blameFile, openFileHistory, openBlame, selectHistoryOid, close: closeFileView } = fileInspection;
  const { commits, historyLoading, hasMore, loadMoreHistory } = history;
  const { pending, setPending, confirmPending, busyOperations, toasts, dismissToast, run } = operations;

  const selected = repositories.find((repository) => repository.id === selectedId);
  const selectRepository = useCallback((repositoryId: number) => setSelectedId(repositoryId), []);
  const openRepositoryFile = useCallback((path: string) => {
    if (selectedId) api.openRepositoryFile(selectedId, path).catch((error) => pushLog("error", errorMessage(error)));
  }, [selectedId, pushLog]);

  useEffect(() => {
    api.bootstrap().then((value) => {
      setGit(value.git); setRepositories(value.repositories); setCustomGroups(value.settings.groupOrder ?? []);
      setLeftWidth(value.settings.leftWidth); setRightWidth(value.settings.rightWidth); setOutputHeight(value.settings.outputHeight);
      setLanguage(value.settings.language ?? "en");
    }).catch((error) => { reportError(errorMessage(error)); });
  }, [reportError]);

  useEffect(() => { document.documentElement.lang = language; }, [language]);

  useEffect(() => {
    if (!selectedId) return;
    closeDiff();
    api.watchRepository(selectedId).catch((error) => pushLog("error", errorMessage(error)));
    if (selected?.kind === "workTree") refreshStatus(selectedId); else { statusRequest.current += 1; setSnapshot(undefined); }
  }, [selectedId, selected?.kind, refreshStatus, pushLog]);

  useEffect(() => {
    const openPalette = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault(); setPaletteOpen(true);
      }
    };
    window.addEventListener("keydown", openPalette);
    return () => window.removeEventListener("keydown", openPalette);
  }, []);

  useEffect(() => {
    const preventSystemMenu = (event: MouseEvent) => event.preventDefault();
    window.addEventListener("contextmenu", preventSystemMenu);
    return () => window.removeEventListener("contextmenu", preventSystemMenu);
  }, []);

  useEffect(() => {
    const closeMenu = () => document.querySelector<HTMLDivElement>(".row-menu-popover:popover-open")?.hidePopover();
    window.addEventListener("blur", closeMenu);
    return () => window.removeEventListener("blur", closeMenu);
  }, []);

  const resolveConflict = useCallback((choices: ConflictResolution[]) => {
    if (!conflict) return;
    const document = conflict;
    void run({ type: "resolveConflictBlocks", snapshotId: document.snapshotId, documentId: document.id, path: document.path, choices }, (outcome) => {
      if (outcome === "succeeded") setConflict((current) => current?.id === document.id ? undefined : current);
    });
  }, [conflict, run]);

  const chooseDirectory = async () => {
    const path = await open({ directory: true, multiple: false });
    return typeof path === "string" ? path : undefined;
  };
  const mutateRepository = async (action: () => Promise<RepositorySummary>) => {
    try { const repository = await action(); await refreshRepositories(); setSelectedId(repository.id); }
    catch (error) { reportError(errorMessage(error)); }
  };
  const register = async () => { const path = await chooseDirectory(); if (path) await mutateRepository(() => api.addRepository(path)); };
  const initialize = async () => { const path = await chooseDirectory(); if (path) await mutateRepository(() => api.initializeRepository(path)); };
  const clone = () => showDialog({ title: t("clone"), submitLabel: t("clone"), fields: [{ name: "url", label: t("remoteUrl"), type: "url", required: true }], onSubmit: async ({ url }) => {
    const destination = await chooseDirectory();
    if (!destination) return;
    try {
      await api.cloneRepository(String(url).trim(), destination);
    } catch (error) { reportError(errorMessage(error)); }
  } });

  const updateSelected = async (changes: Partial<{ name: string; group: string; favorite: boolean }>) => {
    if (!selected) return;
    try {
      await api.updateRepository({ id: selected.id, path: selected.path, name: changes.name ?? selected.name, group: changes.group ?? selected.group, favorite: changes.favorite ?? selected.favorite, order: selected.order });
      await refreshRepositories();
    } catch (error) { reportError(errorMessage(error)); }
  };

  const relocateSelected = async () => { const path = await chooseDirectory(); if (path && selectedId) await mutateRepository(() => api.relocateRepository(selectedId, path)); };
  const removeSelected = () => { if (selected) showDialog({ title: t("removeGitDock"), message: `${selected.name} ${t("removeRepositoryConfirm")}`, danger: true, onSubmit: async () => {
    await api.removeRepository(selected.id); setSelectedId(repositories.find((item) => item.id !== selected.id)?.id); await refreshRepositories();
  } }); };
  const selectGit = async () => {
    const path = await open({ directory: false, multiple: false, title: t("selectGit") });
    if (typeof path === "string") setGit(await api.setGitPath(path));
  };
  const forcePush = () => {
    const branch = selected?.branch;
    if (!selectedId || !branch) return;
    showDialog({ title: t("forcePush"), submitLabel: t("forcePush"), fields: [{ name: "remote", label: t("remote"), value: "origin", required: true }], onSubmit: async ({ remote }) => {
      try {
        const value = String(remote).trim();
        const branches = await api.getBranches(selectedId);
        const expectedOid = branches.find((candidate) => candidate.remote && candidate.name === `${value}/${branch}`)?.oid;
        if (!expectedOid) throw new Error(`${t("fetch")} ${value}/${branch} ${t("fetchBeforeForce")}`);
        await run({ type: "forcePushWithLease", remote: value, branch, expectedOid });
      } catch (error) { reportError(errorMessage(error)); }
    } });
  };

  const toggleLanguage = () => {
    const next = language === "en" ? "zh-CN" : "en";
    setLanguage(next);
    api.saveLanguage(next).catch((error) => { reportError(errorMessage(error)); });
  };

  const saveLayoutSize = (side: LayoutSide, value: number) => {
    const next = clampLayout(side, value);
    if (side === "left") setLeftWidth(next); else if (side === "right") setRightWidth(next); else setOutputHeight(next);
    api.saveLayout(side === "left" ? next : leftWidth, side === "right" ? next : rightWidth, side === "output" ? next : outputHeight).catch((error) => reportError(errorMessage(error)));
  };
  const resizeWithKeyboard = (side: LayoutSide, event: React.KeyboardEvent) => {
    const current = side === "left" ? leftWidth : side === "right" ? rightWidth : outputHeight;
    const step = event.shiftKey ? 50 : 10;
    const direction = side === "output" ? (event.key === "ArrowUp" ? 1 : event.key === "ArrowDown" ? -1 : 0) : side === "right" ? (event.key === "ArrowLeft" ? 1 : event.key === "ArrowRight" ? -1 : 0) : event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
    const next = event.key === "Home" ? LAYOUT_LIMITS[side][0] : event.key === "End" ? LAYOUT_LIMITS[side][1] : direction ? current + direction * step : undefined;
    if (next === undefined) return;
    event.preventDefault();
    saveLayoutSize(side, next);
  };
  const beginResize = (side: LayoutSide, event: React.PointerEvent) => {
    event.preventDefault();
    const start = side === "output" ? event.clientY : event.clientX;
    const initial = side === "left" ? leftWidth : side === "right" ? rightWidth : outputHeight;
    let current = initial;
    const move = (next: PointerEvent) => {
      const delta = side === "output" ? start - next.clientY : next.clientX - start;
      current = clampLayout(side, initial + (side === "right" ? -delta : delta));
      if (side === "left") setLeftWidth(current); else if (side === "right") setRightWidth(current); else setOutputHeight(current);
    };
    const stop = () => {
      window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", stop);
      api.saveLayout(side === "left" ? current : leftWidth, side === "right" ? current : rightWidth, side === "output" ? current : outputHeight).catch((error) => reportError(errorMessage(error)));
    };
    window.addEventListener("pointermove", move); window.addEventListener("pointerup", stop, { once: true });
  };
  const selectWorkflowTab = (next: Tab) => { setTab(next); closeDiff(); };
  const moveWorkflowTab = (event: React.KeyboardEvent<HTMLButtonElement>, current: Tab) => {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    const offset = event.key === "ArrowRight" ? 1 : -1;
    const next = WORKFLOW_TABS[(WORKFLOW_TABS.indexOf(current) + offset + WORKFLOW_TABS.length) % WORKFLOW_TABS.length];
    selectWorkflowTab(next);
    document.getElementById(`workflow-tab-${next}`)?.focus();
  };

  const command = (id: string, key: Parameters<typeof translate>[1], action: () => void, enabled = true): CommandItem | undefined => enabled ? { id, label: t(key), search: `${t(key)} ${translate("en", key)}`.toLowerCase(), action } : undefined;
  const commands = [
    command("changes", "changes", () => { setTab("changes"); closeDiff(); }, Boolean(selected)),
    command("history", "history", () => { setTab("history"); closeDiff(); }, Boolean(selected)),
    command("branches", "branches", () => { setTab("branches"); closeDiff(); }, Boolean(selected)),
    command("stashes", "stashes", () => { setTab("stashes"); closeDiff(); }, Boolean(selected)),
    command("add", "addRepository", register, git.supported), command("clone", "clone", clone, git.supported), command("init", "initialize", initialize, git.supported),
    command("refresh", "refreshAll", refreshRepositories), command("language", "language", toggleLanguage), command("git", "selectGit", selectGit),
    command("fetch", "fetch", () => { void run({ type: "fetch", remote: null, prune: false }); }, Boolean(selected?.capabilities.canManageRemotes)),
    command("pull", "pull", () => { void run({ type: "pull", strategy: null }); }, Boolean(selected?.capabilities.canWriteWorkTree)),
    command("pull-merge", "pullMerge", () => { void run({ type: "pull", strategy: "merge" }); }, Boolean(selected?.capabilities.canWriteWorkTree)),
    command("pull-rebase", "pullRebase", () => { void run({ type: "pull", strategy: "rebase" }); }, Boolean(selected?.capabilities.canWriteWorkTree)),
    command("pull-ff", "pullFf", () => { void run({ type: "pull", strategy: "fastForwardOnly" }); }, Boolean(selected?.capabilities.canWriteWorkTree)),
    command("push", "push", () => { void run({ type: "push", remote: null, branch: null }); }, Boolean(selected?.capabilities.canManageRemotes)),
    command("force-push", "forcePush", forcePush, Boolean(selected?.capabilities.canManageRemotes && selected.branch)),
    command("upstream", "setUpstream", () => showDialog({ title: t("setUpstream"), fields: [{ name: "remote", label: t("remote"), value: "origin", required: true }], onSubmit: ({ remote }) => { if (selected?.branch) run({ type: "setUpstream", remote: String(remote).trim(), branch: selected.branch }); } }), Boolean(selected?.branch)),
    command("cherry-pick", "cherryPickCommits", () => showDialog({ title: t("cherryPickCommits"), fields: [{ name: "commits", label: t("commitOids"), required: true }], onSubmit: ({ commits }) => run({ type: "cherryPick", commits: String(commits).trim().split(/\s+/) }) }), Boolean(selected?.capabilities.canWriteWorkTree)),
    command("interactive-rebase", "interactiveRebase", () => { if (selectedId) setRebaseDialog({ repositoryId: selectedId }); }, Boolean(selected?.capabilities.canWriteWorkTree)),
    command("undo", "undoCommit", () => { void run({ type: "undoLastCommit" }); }, Boolean(selected?.capabilities.canWriteWorkTree)),
    command("rename", "renameEntry", () => showDialog({ title: t("renameEntry"), fields: [{ name: "name", label: t("repositoryName"), value: selected?.name, required: true }], onSubmit: ({ name }) => updateSelected({ name: String(name).trim() }) }), Boolean(selected)),
    command("favorite", selected?.favorite ? "removeFavorite" : "addFavorite", () => { void updateSelected({ favorite: !selected?.favorite }); }, Boolean(selected)),
    command("group", "setGroup", () => showDialog({ title: t("setGroup"), fields: [{ name: "group", label: t("group"), value: selected?.group ?? "" }], onSubmit: ({ group }) => updateSelected({ group: String(group).trim() }) }), Boolean(selected)),
    command("relocate", "relocate", relocateSelected, Boolean(selected)), command("remove", "removeGitDock", removeSelected, Boolean(selected)),
  ].filter((item): item is CommandItem => Boolean(item));

  const outputPanel = <section className={`output-panel ${outputOpen ? "open" : ""}`}>
    <button className="output-handle" onClick={() => setOutputOpen((value) => !value)}><span>{t("gitOutput")}</span><span>{busyOperations.length ? `${busyOperations.length} ${t("running")}` : `${logCount} ${t("lines")}`} {outputOpen ? "⌄" : "⌃"}</span></button>
    {outputOpen && <><div className="resize-handle resize-output" role="separator" tabIndex={0} aria-label={t("resizeOutput")} aria-orientation="horizontal" aria-valuemin={LAYOUT_LIMITS.output[0]} aria-valuemax={LAYOUT_LIMITS.output[1]} aria-valuenow={outputHeight} onKeyDown={(event) => resizeWithKeyboard("output", event)} onPointerDown={(event) => beginResize("output", event)} /><div className="log" role="log" aria-live="polite" aria-relevant="additions" style={{ height: outputHeight }}><div className="log-toolbar">{busyOperations.map((id) => <button key={id} onClick={() => api.cancelOperation(id)}>{t("cancel")} #{id}</button>)}<button disabled={!logCount} onClick={exportLogs}>{t("exportLog")}</button><button onClick={clearLogs}>{t("clear")}</button></div>{readLogs(logBuffer.current).map((line) => <div key={line.id} className={`log-${line.kind}`}><time>{line.timestamp}</time> {line.message}</div>)}</div></>}
  </section>;
  const toastStack = <ToastStack toasts={toasts} onDismiss={dismissToast} />;

  if (!repositories.length) return <I18nProvider language={language}><><main className="empty-workspace"><EmptyState git={git} onAdd={register} onClone={clone} onInit={initialize} onSelectGit={selectGit} onToggleLanguage={toggleLanguage} lastLog={lastLog} />{outputPanel}</main>{toastStack}{paletteOpen && <CommandPalette items={commands} onClose={() => setPaletteOpen(false)} />}{dialog && <FormDialog spec={dialog} onClose={() => setDialog(undefined)} />}</></I18nProvider>;

  return (
    <I18nProvider language={language}><div className="app-shell" style={{ gridTemplateColumns: `${leftWidth}px 1fr` }}>
      <aside className="repo-sidebar">
        <header className="brand" data-tauri-drag-region><RailMark /><div><strong>GitDock</strong><span>{git.supported ? `Git ${git.version}` : t("gitUnavailable")}</span></div></header>
        <label className="search"><span aria-hidden="true">⌕</span><input name="repositorySearch" autoComplete="off" aria-label={t("searchRepositories")} placeholder={t("findRepository")} value={filter} onChange={(event) => setFilter(event.target.value)} /></label>
        <div className="repo-list" role="list" aria-label={t("repositories")} onDragLeave={(event) => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) clearRepositoryDropHint(); }} onDragEnd={clearRepositoryDropHint}>
          {repositoryGroups.map((group) => <section role="group" aria-label={group.label} className={`repo-group ${!collapsedGroups.has(group.key) && !group.repositories.length ? "empty" : ""}`} key={group.key} onDragEnter={acceptRepositoryDrop} onDragOver={hintRepositoryDrop} onDragLeave={(event) => { if (dropTargetGroup.current === event.currentTarget && !event.currentTarget.contains(event.relatedTarget as Node | null)) clearRepositoryDropHint(); }} onDrop={(event) => {
            event.preventDefault();
            const repositoryId = Number(event.dataTransfer.getData("text/plain")) || draggingRepositoryId.current;
            const row = (event.target as Element).closest<HTMLElement>(".repo-row-shell");
            const targetId = row ? Number(row.dataset.repositoryId) : undefined;
            const bounds = row?.getBoundingClientRect();
            if (!filter.trim() && repositoryId) moveRepository(repositoryId, group.key, targetId, Boolean(bounds && event.clientY >= bounds.top + bounds.height / 2));
            draggingRepositoryId.current = undefined;
            clearRepositoryDropHint();
          }}>
            <header><button aria-expanded={!collapsedGroups.has(group.key)} onClick={() => setCollapsedGroups((current) => { const next = new Set(current); if (next.has(group.key)) next.delete(group.key); else next.add(group.key); return next; })}><span className="group-label">{group.label}<span className="group-chevron" aria-hidden="true">{collapsedGroups.has(group.key) ? "▸" : "▾"}</span></span><code>{group.repositories.length}</code></button>{group.key !== FAVORITES_GROUP && group.key !== UNGROUPED_GROUP && <RowMenu><button onClick={() => showDialog({ title: t("renameGroup"), fields: [{ name: "group", label: t("group"), value: group.label, required: true }], onSubmit: ({ group: value }) => updateGroup(group.key, String(value).trim()) })}>{t("rename")}</button><button onClick={() => updateGroup(group.key)}>{t("ungroup")}</button></RowMenu>}</header>
            {!collapsedGroups.has(group.key) && group.repositories.map((repository, index) => <MemoRepositoryRow key={repository.id} repository={repository} selected={repository.id === selectedId} draggable={!filter.trim()} canMoveUp={!filter.trim() && index > 0} canMoveDown={!filter.trim() && index < group.repositories.length - 1} onSelect={selectRepository} onMove={(direction) => moveRepositoryBy(repository.id, direction)} onDragStart={(event) => { event.dataTransfer.effectAllowed = "move"; event.dataTransfer.setData("text/plain", String(repository.id)); const image = event.currentTarget.querySelector<HTMLElement>(".repo-name"); if (image) event.dataTransfer.setDragImage(image, 12, 12); draggingRepositoryId.current = repository.id; }} onDragEnd={() => { draggingRepositoryId.current = undefined; clearRepositoryDropHint(); }} />)}
          </section>)}
        </div>
        <footer className="sidebar-actions"><button onClick={register}>{t("add")}</button><button onClick={clone}>{t("clone")}</button><button onClick={initialize}>{t("init")}</button><button onClick={() => showDialog({ title: t("addGroup"), fields: [{ name: "group", label: t("group"), required: true }], onSubmit: ({ group }) => addGroup(String(group).trim()) })}>{t("addGroup")}</button><button onClick={toggleLanguage}>{t("language")}</button></footer><div className="resize-handle resize-left" role="separator" tabIndex={0} aria-label={t("resizeRepositories")} aria-orientation="vertical" aria-valuemin={LAYOUT_LIMITS.left[0]} aria-valuemax={LAYOUT_LIMITS.left[1]} aria-valuenow={leftWidth} onKeyDown={(event) => resizeWithKeyboard("left", event)} onPointerDown={(event) => beginResize("left", event)} />
      </aside>

      <main className="workspace">
        <header className="topbar" data-tauri-drag-region>
          <div className="repo-context"><span className="branch-dot" aria-hidden="true" /> <strong>{selected?.name}</strong><code>{selected?.branch || shortOid(selected?.headOid)}</code></div>
          <nav role="tablist" aria-label={t("workflows")}>{WORKFLOW_TABS.map((item) => <button id={`workflow-tab-${item}`} role="tab" aria-selected={tab === item} aria-controls="workflow-panel" tabIndex={tab === item ? 0 : -1} key={item} className={tab === item ? "active" : ""} onKeyDown={(event) => moveWorkflowTab(event, item)} onClick={() => selectWorkflowTab(item)}>{t(item)}</button>)}</nav>
          <div className="sync-actions"><button aria-label={t("commandPalette")} onClick={() => setPaletteOpen(true)}>⌘K</button><button disabled={!selected?.capabilities.canManageRemotes} onClick={() => run({ type: "fetch", remote: null, prune: false })}>{t("fetch")}</button><button disabled={!selected?.capabilities.canWriteWorkTree} onClick={() => run({ type: "pull", strategy: null })}>{t("pull")}</button><button className="primary" disabled={!selected?.capabilities.canManageRemotes} onClick={() => run({ type: "push", remote: null, branch: null })}>{t("push")}</button><RowMenu label={t("more")}><button onClick={refreshRepositories}>{t("refreshAll")}</button><button onClick={() => run({ type: "pull", strategy: "merge" })}>{t("pullMerge")}</button><button onClick={() => run({ type: "pull", strategy: "rebase" })}>{t("pullRebase")}</button><button onClick={() => run({ type: "pull", strategy: "fastForwardOnly" })}>{t("pullFf")}</button><button onClick={forcePush}>{t("forcePush")}</button><button onClick={() => showDialog({ title: t("setUpstream"), fields: [{ name: "remote", label: t("remote"), value: "origin", required: true }], onSubmit: ({ remote }) => { if (selected?.branch) run({ type: "setUpstream", remote: String(remote).trim(), branch: selected.branch }); } })}>{t("setUpstream")}</button><button onClick={() => run({ type: "undoLastCommit" })}>{t("undoCommit")}</button><button onClick={() => showDialog({ title: t("renameEntry"), fields: [{ name: "name", label: t("repositoryName"), value: selected?.name, required: true }], onSubmit: ({ name }) => updateSelected({ name: String(name).trim() }) })}>{t("renameEntry")}</button><button onClick={relocateSelected}>{t("relocate")}</button><button onClick={selectGit}>{t("selectGit")}</button><button className="menu-danger" onClick={removeSelected}>{t("removeGitDock")}</button></RowMenu></div>
        </header>

        <div id="workflow-panel" className="work-area" role="tabpanel" aria-labelledby={`workflow-tab-${tab}`} style={{ gridTemplateColumns: `minmax(480px, 1fr) ${rightWidth}px` }}>
          <section className="canvas">
            {conflict ? <ConflictEditor key={conflict.id} document={conflict} onBack={closeDiff} onResolve={resolveConflict} /> : fileView === "history" && filePath ? <FileHistoryView path={filePath} entries={fileHistoryEntries} selectedOid={fileHistoryOid} diff={fileDiff} onBack={closeFileView} onSelect={selectHistoryOid} /> : fileView === "blame" && blameFile ? <BlameView blame={blameFile} onBack={closeFileView} /> : diff ? <MemoDiffView diff={diff} snapshotId={snapshot?.id} onBack={commitDetail ? closeCommitFile : closeDiff} onRun={run} onHunkSettled={reloadOpenDiff} fileActions={diffIsFile} onFileHistory={openFileHistory} onBlame={openBlame} caption={commitDetail ? shortOid(commitDetail.oid) : undefined} /> : commitDetail ? <CommitDetailView detail={commitDetail} onBack={closeDiff} onOpenFile={openCommitFile} /> : !selected ? <div className="canvas-empty"><h2>{t("selectRepository")}</h2><p>{t("selectRepositoryHint")}</p></div> : tab === "changes" ? <MemoChangesOverview repository={selected} snapshot={snapshot} /> : tab === "history" ? <MemoHistoryCanvas commits={commits} selectedOid={selectedCommit} onSelect={openCommit} /> : tab === "branches" ? <BranchCanvas repository={selected} /> : <StashCanvas repository={selected} />}
          </section>
          <aside className="tool-pane"><div className="resize-handle resize-right" role="separator" tabIndex={0} aria-label={t("resizeDetails")} aria-orientation="vertical" aria-valuemin={LAYOUT_LIMITS.right[0]} aria-valuemax={LAYOUT_LIMITS.right[1]} aria-valuenow={rightWidth} onKeyDown={(event) => resizeWithKeyboard("right", event)} onPointerDown={(event) => beginResize("right", event)} />
            {tab === "changes" && <MemoChangesPane repository={selected} snapshot={snapshot} onOpen={openDiff} onOpenExternal={openRepositoryFile} onLoadIgnored={loadIgnored} onRun={run} onFileHistory={openFileHistory} onBlame={openBlame} />}
            {tab === "history" && <MemoHistoryPane commits={commits} selectedOid={selectedCommit} loading={historyLoading} hasMore={hasMore} onLoadMore={loadMoreHistory} onSelect={openCommit} onRun={run} />}
            {tab === "branches" && selectedId && <MemoBranchesPane repositoryId={selectedId} onRun={run} onDialog={showDialog} onDiff={showBranchDiff} onError={reportError} onInteractiveRebase={(onto) => selectedId && setRebaseDialog({ repositoryId: selectedId, onto })} />}
            {tab === "stashes" && selectedId && <MemoStashesPane repositoryId={selectedId} onRun={run} onDialog={showDialog} onError={reportError} />}
          </aside>
        </div>

        {outputPanel}
      </main>

      {pending && <ConfirmDialog pending={pending} onCancel={() => { pending.onFinished?.("cancelled"); setPending(undefined); }} onConfirm={confirmPending} />}
      {toastStack}
      {paletteOpen && <CommandPalette items={commands} onClose={() => setPaletteOpen(false)} />}
      {rebaseDialog && <InteractiveRebase repositoryId={rebaseDialog.repositoryId} initialOnto={rebaseDialog.onto} onClose={() => setRebaseDialog(undefined)} onRun={run} />}
      {dialog && <FormDialog spec={dialog} onClose={() => setDialog(undefined)} />}
    </div></I18nProvider>
  );
}
