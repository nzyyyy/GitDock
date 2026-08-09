import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import { api, type BranchInfo, type CommitInfo, type DiffFile, type FileChange, type GitInfo, type Language, type OperationEvent, type OperationPreview, type OperationRequest, type RemoteInfo, type RepositorySummary, type StashInfo, type SubmoduleInfo, type TagInfo, type WorkingTreeSnapshot } from "./api";
import { I18nProvider, translate, useI18n } from "./i18n";

type Tab = "changes" | "history" | "branches" | "stashes";
type LogLine = { id: number; kind: OperationEvent["kind"] | "error"; message: string };
type Pending = { request: OperationRequest; preview: OperationPreview };

const shortOid = (oid?: string) => oid?.slice(0, 8) ?? "—";
const errorMessage = (error: unknown) => error instanceof Error ? error.message : String(error);

export default function App() {
  const [git, setGit] = useState<GitInfo>({ supported: false });
  const [repositories, setRepositories] = useState<RepositorySummary[]>([]);
  const [selectedId, setSelectedId] = useState<number>();
  const [tab, setTab] = useState<Tab>("changes");
  const [snapshot, setSnapshot] = useState<WorkingTreeSnapshot>();
  const [diff, setDiff] = useState<DiffFile>();
  const [logs, setLogs] = useState<LogLine[]>([]);
  const [outputOpen, setOutputOpen] = useState(false);
  const [pending, setPending] = useState<Pending>();
  const [busyOperations, setBusyOperations] = useState<number[]>([]);
  const [filter, setFilter] = useState("");
  const [leftWidth, setLeftWidth] = useState(240);
  const [rightWidth, setRightWidth] = useState(360);
  const [outputHeight, setOutputHeight] = useState(190);
  const [language, setLanguage] = useState<Language>("en");
  const allowClose = useRef(false);
  const t = (key: Parameters<typeof translate>[1]) => translate(language, key);

  const selected = repositories.find((repository) => repository.id === selectedId);
  const pushLog = useCallback((kind: LogLine["kind"], message: string) => {
    setLogs((current) => [...current.slice(-299), { id: Date.now() + Math.random(), kind, message }]);
  }, []);

  const refreshRepositories = useCallback(async () => {
    try { setRepositories(await api.refreshRepositories()); }
    catch (error) { pushLog("error", errorMessage(error)); setOutputOpen(true); }
  }, [pushLog]);

  const refreshStatus = useCallback(async (repositoryId = selectedId, includeIgnored = false) => {
    if (!repositoryId) return;
    try { setSnapshot(await api.status(repositoryId, includeIgnored)); }
    catch (error) { setSnapshot(undefined); pushLog("error", errorMessage(error)); setOutputOpen(true); }
  }, [selectedId, pushLog]);

  useEffect(() => {
    api.bootstrap().then((value) => {
      setGit(value.git); setRepositories(value.repositories);
      setSelectedId(value.settings.selectedRepositoryId ?? value.repositories[0]?.id);
      setLeftWidth(value.settings.leftWidth); setRightWidth(value.settings.rightWidth); setOutputHeight(value.settings.outputHeight);
      setLanguage(value.settings.language ?? "en");
    }).catch((error) => { pushLog("error", errorMessage(error)); setOutputOpen(true); });
  }, [pushLog]);

  useEffect(() => { document.documentElement.lang = language; }, [language]);

  useEffect(() => {
    if (!selectedId) return;
    setDiff(undefined);
    api.watchRepository(selectedId).catch((error) => pushLog("error", errorMessage(error)));
    if (selected?.kind === "workTree") refreshStatus(selectedId); else setSnapshot(undefined);
  }, [selectedId, selected?.kind, refreshStatus, pushLog]);

  useEffect(() => {
    const unlisteners = Promise.all([
      listen<OperationEvent>("operation-event", ({ payload }) => {
        pushLog(payload.kind, payload.message);
        if (payload.kind === "started") setBusyOperations((ids) => [...ids, payload.operationId]);
        if (payload.kind === "finished") {
          setBusyOperations((ids) => ids.filter((id) => id !== payload.operationId));
          if ((payload.exitCode ?? 1) !== 0) setOutputOpen(true);
        }
        if (payload.kind === "stderr") setOutputOpen(true);
      }),
      listen<{ repositoryId: number }>("repository-changed", ({ payload }) => {
        refreshRepositories();
        if (payload.repositoryId === selectedId) refreshStatus(payload.repositoryId);
      }),
      listen("repository-list-changed", refreshRepositories),
    ]);
    return () => { unlisteners.then((values) => values.forEach((unlisten) => unlisten())); };
  }, [pushLog, refreshRepositories, refreshStatus, selectedId]);

  useEffect(() => {
    const listener = getCurrentWindow().onCloseRequested(async (event) => {
      if (allowClose.current || !busyOperations.length) return;
      event.preventDefault();
      if (window.confirm(`${busyOperations.length} ${t("closeOperations")}`)) {
        await Promise.allSettled(busyOperations.map(api.cancel));
        allowClose.current = true;
        await getCurrentWindow().close();
      }
    });
    return () => { listener.then((unlisten) => unlisten()); };
  }, [busyOperations, language]);

  useEffect(() => {
    const closeMenu = () => document.querySelector<HTMLDivElement>(".row-menu-popover:popover-open")?.hidePopover();
    window.addEventListener("blur", closeMenu);
    return () => window.removeEventListener("blur", closeMenu);
  }, []);

  const run = useCallback(async (request: OperationRequest) => {
    if (!selectedId) return;
    try {
      const preview = await api.preview(selectedId, request);
      if (preview.requiresConfirmation) { setPending({ request, preview }); return; }
      await api.start(selectedId, request);
    } catch (error) { pushLog("error", errorMessage(error)); setOutputOpen(true); }
  }, [selectedId, pushLog]);

  const confirmPending = async () => {
    if (!pending || !selectedId) return;
    try { await api.start(selectedId, pending.request, true); setPending(undefined); }
    catch (error) { pushLog("error", errorMessage(error)); setOutputOpen(true); }
  };

  const chooseDirectory = async () => {
    const path = await open({ directory: true, multiple: false });
    return typeof path === "string" ? path : undefined;
  };
  const register = async () => { const path = await chooseDirectory(); if (path) await mutateRepository(() => api.addRepository(path)); };
  const initialize = async () => { const path = await chooseDirectory(); if (path) await mutateRepository(() => api.initRepository(path)); };
  const clone = async () => {
    const url = window.prompt(t("remoteUrl")); if (!url) return;
    const destination = await chooseDirectory(); if (destination) await mutateRepository(() => api.cloneRepository(url, destination));
  };
  const mutateRepository = async (action: () => Promise<RepositorySummary>) => {
    try { const repository = await action(); await refreshRepositories(); setSelectedId(repository.id); }
    catch (error) { pushLog("error", errorMessage(error)); setOutputOpen(true); }
  };

  const updateSelected = async (changes: Partial<{ name: string; group: string; favorite: boolean }>) => {
    if (!selected) return;
    try {
      await api.updateRepository({ id: selected.id, path: selected.path, name: changes.name ?? selected.name, group: changes.group ?? selected.group, favorite: changes.favorite ?? selected.favorite, order: repositories.findIndex((item) => item.id === selected.id) });
      await refreshRepositories();
    } catch (error) { pushLog("error", errorMessage(error)); setOutputOpen(true); }
  };

  const relocateSelected = async () => { const path = await chooseDirectory(); if (path && selectedId) await mutateRepository(() => api.relocateRepository(selectedId, path)); };
  const removeSelected = async () => {
    if (!selected || !window.confirm(`${selected.name} ${t("removeRepositoryConfirm")}`)) return;
    await api.removeRepository(selected.id); setSelectedId(repositories.find((item) => item.id !== selected.id)?.id); await refreshRepositories();
  };
  const selectGit = async () => {
    const path = await open({ directory: false, multiple: false, title: t("selectGit") });
    if (typeof path === "string") setGit(await api.setGitPath(path));
  };
  const forcePush = async () => {
    if (!selectedId || !selected?.branch) return;
    const remote = window.prompt(t("remote"), "origin"); if (!remote) return;
    try {
      const branches = await api.branches(selectedId);
      const expectedOid = branches.find((branch) => branch.remote && branch.name === `${remote}/${selected.branch}`)?.oid;
      if (!expectedOid) throw new Error(`${t("fetch")} ${remote}/${selected.branch} ${t("fetchBeforeForce")}`);
      await run({ type: "forcePushWithLease", remote, branch: selected.branch, expectedOid });
    } catch (error) { pushLog("error", errorMessage(error)); setOutputOpen(true); }
  };

  const toggleLanguage = () => {
    const next = language === "en" ? "zh-CN" : "en";
    setLanguage(next);
    api.saveLanguage(next).catch((error) => { pushLog("error", errorMessage(error)); setOutputOpen(true); });
  };

  const beginResize = (side: "left" | "right" | "output", event: React.PointerEvent) => {
    event.preventDefault();
    const start = side === "output" ? event.clientY : event.clientX;
    const initial = side === "left" ? leftWidth : side === "right" ? rightWidth : outputHeight;
    let current = initial;
    const move = (next: PointerEvent) => {
      const delta = side === "output" ? start - next.clientY : next.clientX - start;
      current = Math.max(side === "left" ? 190 : side === "right" ? 300 : 120, Math.min(side === "left" ? 420 : side === "right" ? 560 : 420, initial + (side === "right" ? -delta : delta)));
      if (side === "left") setLeftWidth(current); else if (side === "right") setRightWidth(current); else setOutputHeight(current);
    };
    const stop = () => {
      window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", stop);
      api.saveLayout(side === "left" ? current : leftWidth, side === "right" ? current : rightWidth, side === "output" ? current : outputHeight).catch((error) => pushLog("error", errorMessage(error)));
    };
    window.addEventListener("pointermove", move); window.addEventListener("pointerup", stop, { once: true });
  };

  const openDiff = async (file: FileChange, staged: boolean) => {
    if (!selectedId || !snapshot) return;
    try { setDiff(await api.diff(selectedId, snapshot.id, file.path, staged)); }
    catch (error) { pushLog("error", errorMessage(error)); setOutputOpen(true); }
  };

  const visibleRepositories = useMemo(() => repositories.filter((repo) => `${repo.name} ${repo.path} ${repo.group ?? ""}`.toLowerCase().includes(filter.toLowerCase())).sort((a, b) => Number(b.favorite) - Number(a.favorite) || a.name.localeCompare(b.name)), [repositories, filter]);

  if (!repositories.length) return <I18nProvider language={language}><EmptyState git={git} onAdd={register} onClone={clone} onInit={initialize} onSelectGit={selectGit} onToggleLanguage={toggleLanguage} logs={logs} /></I18nProvider>;

  return (
    <I18nProvider language={language}><div className="app-shell" style={{ gridTemplateColumns: `${leftWidth}px 1fr` }}>
      <aside className="repo-sidebar">
        <header className="brand"><RailMark /><div><strong>GitDock</strong><span>{git.supported ? `Git ${git.version}` : t("gitUnavailable")}</span></div></header>
        <label className="search"><span>⌕</span><input aria-label={t("searchRepositories")} placeholder={t("findRepository")} value={filter} onChange={(event) => setFilter(event.target.value)} /></label>
        <div className="repo-list" role="listbox" aria-label={t("repositories")}>
          {visibleRepositories.map((repository) => <RepositoryRow key={repository.id} repository={repository} selected={repository.id === selectedId} onSelect={() => setSelectedId(repository.id)} />)}
        </div>
        <footer className="sidebar-actions"><button onClick={register}>{t("add")}</button><button onClick={clone}>{t("clone")}</button><button onClick={initialize}>{t("init")}</button><button onClick={toggleLanguage}>{t("language")}</button></footer><div className="resize-handle resize-left" onPointerDown={(event) => beginResize("left", event)} />
      </aside>

      <main className="workspace">
        <header className="topbar">
          <div className="repo-context"><span className="branch-dot" /> <strong>{selected?.name}</strong><code>{selected?.branch || shortOid(selected?.headOid)}</code></div>
          <nav aria-label={t("workflows")}>{(["changes", "history", "branches", "stashes"] as Tab[]).map((item) => <button key={item} className={tab === item ? "active" : ""} onClick={() => { setTab(item); setDiff(undefined); }}>{t(item)}</button>)}</nav>
          <div className="sync-actions"><button disabled={!selected?.capabilities.canManageRemotes} onClick={() => run({ type: "fetch", prune: false })}>{t("fetch")}</button><button disabled={!selected?.capabilities.canWriteWorkTree} onClick={() => run({ type: "pull" })}>{t("pull")}</button><button className="primary" disabled={!selected?.capabilities.canManageRemotes} onClick={() => run({ type: "push" })}>{t("push")}</button><RowMenu label={t("more")}><button onClick={refreshRepositories}>{t("refreshAll")}</button><button onClick={() => run({ type: "pull", strategy: "merge" })}>{t("pullMerge")}</button><button onClick={() => run({ type: "pull", strategy: "rebase" })}>{t("pullRebase")}</button><button onClick={() => run({ type: "pull", strategy: "fastForwardOnly" })}>{t("pullFf")}</button><button onClick={forcePush}>{t("forcePush")}</button><button onClick={() => { const remote = window.prompt(t("remote"), "origin"); const branch = remote && selected?.branch; if (remote && branch) run({ type: "setUpstream", remote, branch }); }}>{t("setUpstream")}</button><button onClick={() => { const commits = window.prompt(t("commitOids"))?.trim().split(/\s+/); if (commits?.length) run({ type: "cherryPick", commits }); }}>{t("cherryPickCommits")}</button><button onClick={() => run({ type: "undoLastCommit" })}>{t("undoCommit")}</button><button onClick={() => { const name = window.prompt(t("repositoryName"), selected?.name); if (name) updateSelected({ name }); }}>{t("renameEntry")}</button><button onClick={() => updateSelected({ favorite: !selected?.favorite })}>{selected?.favorite ? t("removeFavorite") : t("addFavorite")}</button><button onClick={() => { const group = window.prompt(t("group"), selected?.group ?? ""); if (group !== null) updateSelected({ group }); }}>{t("setGroup")}</button><button onClick={relocateSelected}>{t("relocate")}</button><button onClick={selectGit}>{t("selectGit")}</button><button className="menu-danger" onClick={removeSelected}>{t("removeGitDock")}</button></RowMenu></div>
        </header>

        <div className="work-area" style={{ gridTemplateColumns: `minmax(480px, 1fr) ${rightWidth}px` }}>
          <section className="canvas">
            {diff ? <DiffView diff={diff} snapshotId={snapshot?.id} onBack={() => setDiff(undefined)} onRun={run} /> : tab === "changes" ? <ChangesOverview repository={selected} snapshot={snapshot} /> : tab === "history" ? <HistoryCanvas repositoryId={selectedId!} onError={(message) => { pushLog("error", message); setOutputOpen(true); }} /> : tab === "branches" ? <BranchCanvas repository={selected} /> : <StashCanvas repository={selected} />}
          </section>
          <aside className="tool-pane"><div className="resize-handle resize-right" onPointerDown={(event) => beginResize("right", event)} />
            {tab === "changes" && <ChangesPane repository={selected} snapshot={snapshot} onOpen={openDiff} onOpenExternal={(path) => api.openRepositoryFile(selectedId!, path).catch((error) => pushLog("error", errorMessage(error)))} onLoadIgnored={() => refreshStatus(selectedId, true)} onRun={run} />}
            {tab === "history" && <HistoryPane repositoryId={selectedId!} onRun={run} onDiff={(value) => setDiff({ path: t("commitDiff"), staged: false, binary: false, tooLarge: false, patch: value, hunks: [] })} onError={(message) => { pushLog("error", message); setOutputOpen(true); }} />}
            {tab === "branches" && <BranchesPane repositoryId={selectedId!} onRun={run} onDiff={(value) => setDiff({ path: t("branchComparison"), staged: false, binary: false, tooLarge: false, patch: value, hunks: [] })} onError={(message) => pushLog("error", message)} />}
            {tab === "stashes" && <StashesPane repositoryId={selectedId!} onRun={run} onError={(message) => pushLog("error", message)} />}
          </aside>
        </div>

        <section className={`output-panel ${outputOpen ? "open" : ""}`}>
          <button className="output-handle" onClick={() => setOutputOpen((value) => !value)}><span>{t("gitOutput")}</span><span>{busyOperations.length ? `${busyOperations.length} ${t("running")}` : `${logs.length} ${t("lines")}`} {outputOpen ? "⌄" : "⌃"}</span></button>
          {outputOpen && <><div className="resize-handle resize-output" onPointerDown={(event) => beginResize("output", event)} /><div className="log" style={{ height: outputHeight }}><div className="log-toolbar">{busyOperations.map((id) => <button key={id} onClick={() => api.cancel(id)}>{t("cancel")} #{id}</button>)}<button onClick={() => setLogs([])}>{t("clear")}</button></div>{logs.map((line) => <div key={line.id} className={`log-${line.kind}`}>{line.message}</div>)}</div></>}
        </section>
      </main>

      {pending && <ConfirmDialog pending={pending} onCancel={() => setPending(undefined)} onConfirm={confirmPending} />}
    </div></I18nProvider>
  );
}

function EmptyState({ git, onAdd, onClone, onInit, onSelectGit, onToggleLanguage, logs }: { git: GitInfo; onAdd: () => void; onClone: () => void; onInit: () => void; onSelectGit: () => void; onToggleLanguage: () => void; logs: LogLine[] }) {
  const { t } = useI18n();
  return <main className="empty-state"><div className="empty-brand"><RailMark /><span>GITDOCK / WORKSPACE</span></div><h1>{t("emptyTitle1")}<br />{t("emptyTitle2")}</h1><p>{t("emptyDescription")}</p><div className="empty-actions"><button className="primary" disabled={!git.supported} onClick={onAdd}>{t("addRepository")}</button><button disabled={!git.supported} onClick={onClone}>{t("clone")}</button><button disabled={!git.supported} onClick={onInit}>{t("initialize")}</button>{!git.supported && <button onClick={onSelectGit}>{t("selectGit")}</button>}<button onClick={onToggleLanguage}>{t("language")}</button></div><div className={`git-check ${git.supported ? "ok" : "bad"}`}><span>{git.supported ? "●" : "×"}</span><div><strong>{git.supported ? `Git ${git.version}` : t("gitRequired")}</strong><small>{git.path ?? git.error}</small></div></div>{logs.at(-1) && <p className="empty-error">{logs.at(-1)?.message}</p>}</main>;
}

function RailMark() { return <svg className="rail-mark" viewBox="0 0 32 32" aria-hidden="true"><path d="M9 4v18a6 6 0 0 0 6 6h3" /><path d="M23 4v7a5 5 0 0 1-5 5H9" /><circle cx="9" cy="4" r="2.5" /><circle cx="23" cy="4" r="2.5" /><circle cx="20" cy="28" r="2.5" /></svg>; }

function RepositoryRow({ repository, selected, onSelect }: { repository: RepositorySummary; selected: boolean; onSelect: () => void }) {
  const { t } = useI18n();
  const state = repository.kind === "missing" ? "missing" : repository.conflictCount ? "conflict" : repository.changedCount ? "changed" : "clean";
  return <button role="option" aria-selected={selected} className={`repo-row ${selected ? "selected" : ""}`} onClick={onSelect}><span className={`status-rail ${state}`} /><span className="repo-copy"><span className="repo-name">{repository.favorite && "★ "}{repository.name}<i>{repository.conflictCount ? `${repository.conflictCount} ${t("conflicts")}` : t(state)}</i></span><span className="repo-meta"><code>{repository.branch || shortOid(repository.headOid)}</code><span>{repository.changedCount ? `±${repository.changedCount}` : t("clean")}</span>{(repository.ahead || repository.behind) ? <span>↑{repository.ahead} ↓{repository.behind}</span> : null}</span></span></button>;
}

function ChangesOverview({ repository, snapshot }: { repository?: RepositorySummary; snapshot?: WorkingTreeSnapshot }) {
  const { t } = useI18n();
  const changed = snapshot?.files.filter((file) => !file.ignored).length ?? 0;
  if (!changed) return <div className="canvas-empty"><span className="large-check">✓</span><h2>{t("workingTreeClean")}</h2><p>{repository?.lastCommit || t("noLocalChanges")}</p></div>;
  return <div className="canvas-empty"><div className="change-tally"><strong>{changed}</strong><span>{t("workingTreeChanges")}</span></div><h2>{t("selectFile")}</h2><p>{t("inspectHint")}</p></div>;
}

function ChangesPane({ repository, snapshot, onOpen, onOpenExternal, onLoadIgnored, onRun }: { repository?: RepositorySummary; snapshot?: WorkingTreeSnapshot; onOpen: (file: FileChange, staged: boolean) => void; onOpenExternal: (path: string) => void; onLoadIgnored: () => void; onRun: (request: OperationRequest) => void }) {
  const { t } = useI18n();
  const [message, setMessage] = useState(""); const [amend, setAmend] = useState(false); const [signoff, setSignoff] = useState(false);
  const [stageSelection, setStageSelection] = useState<string[]>([]); const [unstageSelection, setUnstageSelection] = useState<string[]>([]);
  const files = snapshot?.files ?? [];
  const groups = [
    [t("conflictsGroup"), files.filter((f) => f.conflict), "conflict"],
    [t("staged"), files.filter((f) => f.staged && !f.conflict), "staged"],
    [t("unstaged"), files.filter((f) => f.unstaged && !f.conflict && f.kind !== "Untracked" && !f.ignored), "unstaged"],
    [t("untracked"), files.filter((f) => f.kind === "Untracked"), "untracked"],
    [t("ignored"), files.filter((f) => f.ignored), "ignored"],
  ] as const;
  const validStagePaths = new Set(groups.filter(([, , type]) => type === "unstaged" || type === "untracked").flatMap(([, entries]) => entries.map((file) => file.path)));
  const validUnstagePaths = new Set(groups.filter(([, , type]) => type === "staged").flatMap(([, entries]) => entries.map((file) => file.path)));
  useEffect(() => {
    setStageSelection((current) => current.filter((path) => validStagePaths.has(path)));
    setUnstageSelection((current) => current.filter((path) => validUnstagePaths.has(path)));
  }, [snapshot?.id, repository?.id]);
  const toggle = (path: string, selected: string[], setSelected: React.Dispatch<React.SetStateAction<string[]>>) => setSelected(selected.includes(path) ? selected.filter((item) => item !== path) : [...selected, path]);
  const batch = (type: "stageFiles" | "unstageFiles", paths: string[], clear: () => void) => { onRun({ type, paths }); clear(); };
  return <div className="changes-pane"><div className="pane-title"><span>{t("workingTree")}</span><span className="batch-actions">{stageSelection.length > 0 && <button onClick={() => batch("stageFiles", stageSelection, () => setStageSelection([]))}>{t("stageSelected")} ({stageSelection.length})</button>}{unstageSelection.length > 0 && <button onClick={() => batch("unstageFiles", unstageSelection, () => setUnstageSelection([]))}>{t("unstageSelected")} ({unstageSelection.length})</button>}{stageSelection.length === 0 && unstageSelection.length === 0 && <button onClick={onLoadIgnored}>{t("loadIgnored")}</button>}</span></div><div className="change-groups">{repository?.ongoing && <div className="ongoing"><strong>{repository.ongoing.kind} {t("inProgress")}</strong>{repository.ongoing.canContinue && <button onClick={() => onRun({ type: "continue", kind: repository.ongoing!.kind })}>{t("continue")}</button>}{repository.ongoing.canSkip && <button onClick={() => onRun({ type: "skip", kind: repository.ongoing!.kind })}>{t("skip")}</button>}{repository.ongoing.canAbort && <button onClick={() => onRun({ type: "abort", kind: repository.ongoing!.kind })}>{t("abort")}</button>}</div>}{groups.map(([name, entries, type]) => { const selected = type === "staged" ? unstageSelection : stageSelection; const setSelected = type === "staged" ? setUnstageSelection : setStageSelection; return <ChangeGroup key={type} name={name} files={entries} type={type} selected={selected} onToggle={(path) => toggle(path, selected, setSelected)} onSelectAll={() => setSelected(entries.every((file) => selected.includes(file.path)) ? selected.filter((path) => !entries.some((file) => file.path === path)) : [...new Set([...selected, ...entries.map((file) => file.path)])])} onOpen={onOpen} onOpenExternal={onOpenExternal} onRun={onRun} />; })}</div><form className="commit-box" onSubmit={(event) => { event.preventDefault(); onRun({ type: "commit", message, amend, signoff }); }}><label>{t("commitMessage")}<textarea value={message} onChange={(event) => setMessage(event.target.value)} placeholder={t("commitPlaceholder")} /></label><div className="commit-options"><label><input type="checkbox" checked={amend} onChange={(event) => setAmend(event.target.checked)} /> {t("amend")}</label><label><input type="checkbox" checked={signoff} onChange={(event) => setSignoff(event.target.checked)} /> {t("signOff")}</label></div><button className="primary" disabled={!message.trim()}>{t("commitStaged")}</button></form></div>;
}

function RowMenu({ children, label }: { children: React.ReactNode; label?: string }) {
  const { t } = useI18n();
  const actualLabel = label ?? t("moreActions");
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const toggle = () => {
    const button = buttonRef.current; const menu = menuRef.current;
    if (!button || !menu) return;
    if (open) { menu.hidePopover(); return; }
    menu.showPopover();
    menu.style.height = "0";
    menu.style.height = `${Math.min(menu.scrollHeight + 2, window.innerHeight - 8)}px`;
    const anchor = button.getBoundingClientRect(); const bounds = menu.getBoundingClientRect();
    const top = anchor.bottom + bounds.height + 4 <= window.innerHeight ? anchor.bottom + 4 : Math.max(4, anchor.top - bounds.height - 4);
    menu.style.left = `${Math.max(4, Math.min(anchor.right - bounds.width, window.innerWidth - bounds.width - 4))}px`;
    menu.style.top = `${top}px`;
  };
  return <><button ref={buttonRef} className="row-menu-trigger" type="button" aria-label={actualLabel} aria-haspopup="menu" aria-expanded={open} onClick={toggle}>{label ? actualLabel : "•••"}</button><div ref={menuRef} className="row-menu-popover" popover="auto" role="menu" onToggle={(event) => setOpen(event.newState === "open")} onClick={(event) => { if ((event.target as HTMLElement).closest("button")) menuRef.current?.hidePopover(); }}>{children}</div></>;
}

function ChangeGroup({ name, files, type, selected, onToggle, onSelectAll, onOpen, onOpenExternal, onRun }: { name: string; files: FileChange[]; type: string; selected: string[]; onToggle: (path: string) => void; onSelectAll: () => void; onOpen: (file: FileChange, staged: boolean) => void; onOpenExternal: (path: string) => void; onRun: (request: OperationRequest) => void }) {
  const { t } = useI18n();
  if (!files.length) return null;
  const selectable = type === "staged" || type === "unstaged" || type === "untracked";
  return <section className="change-group"><header><span>{selectable && <input type="checkbox" aria-label={`${t("selectAll")} ${name}`} checked={files.every((file) => selected.includes(file.path))} onChange={onSelectAll} />}{name}</span><code>{files.length}</code></header>{files.map((file) => <div className={`file-row ${selectable ? "selectable" : ""} ${type === "conflict" ? "conflict-row" : ""}`} key={`${type}-${file.path}`}>{selectable && <input type="checkbox" aria-label={`${type === "staged" ? t("selectFileForUnstage") : t("selectFileForStage")} ${file.path}`} checked={selected.includes(file.path)} onChange={() => onToggle(file.path)} />}<button className="file-main" onClick={() => onOpen(file, type === "staged")}><b>{file.path.split("/").at(-1)}</b><small>{file.path.includes("/") ? file.path.slice(0, file.path.lastIndexOf("/")) : "./"}</small></button><span className={`file-kind kind-${file.kind.toLowerCase()}`}>{file.kind[0]}</span>{type === "staged" ? <button onClick={() => onRun({ type: "unstageFiles", paths: [file.path] })}>{t("unstage")}</button> : type === "untracked" ? <><button onClick={() => onRun({ type: "stageFiles", paths: [file.path] })}>{t("stage")}</button><button className="danger-icon" aria-label={`${t("trash")} ${file.path}`} onClick={() => onRun({ type: "trashUntracked", paths: [file.path] })}>⌫</button></> : type === "conflict" ? <RowMenu label={t("resolve")}><button onClick={() => onRun({ type: "chooseConflictSide", path: file.path, side: "ours" })}>{t("useCurrent")}</button><button onClick={() => onRun({ type: "chooseConflictSide", path: file.path, side: "theirs" })}>{t("useIncoming")}</button><button onClick={() => onOpenExternal(file.path)}>{t("openExternal")}</button><button onClick={() => onRun({ type: "runMergetool", path: file.path })}>{t("runMergetool")}</button><button onClick={() => onRun({ type: "markResolved", paths: [file.path] })}>{t("markResolved")}</button></RowMenu> : type === "ignored" ? null : <><button onClick={() => onRun({ type: "stageFiles", paths: [file.path] })}>{t("stage")}</button><button className="danger-icon" aria-label={`${t("discard")} ${file.path}`} onClick={() => onRun({ type: "discardTracked", paths: [file.path] })}>↶</button></>}</div>)}</section>;
}

function DiffView({ diff, snapshotId, onBack, onRun }: { diff: DiffFile; snapshotId?: number; onBack: () => void; onRun: (request: OperationRequest) => void }) {
  const { t } = useI18n();
  if (diff.binary || diff.tooLarge) return <div className="diff-view"><header className="canvas-header"><button onClick={onBack}>← {t("back")}</button><strong>{diff.path}</strong></header><div className="canvas-empty"><h2>{diff.binary ? t("binaryDiff") : t("diffTooLarge")}</h2><button onClick={() => onRun({ type: "runDifftool", path: diff.path, staged: diff.staged })}>{t("openDifftool")}</button></div></div>;
  const lines = diff.patch.split("\n");
  return <div className="diff-view"><header className="canvas-header"><button onClick={onBack}>← {t("back")}</button><strong>{diff.path}</strong><span>{diff.staged ? "INDEX ↔ HEAD" : "WORKTREE ↔ INDEX"}</span></header><div className="diff-lines">{lines.map((line, index) => <div key={index} className={line.startsWith("+") && !line.startsWith("+++") ? "add" : line.startsWith("-") && !line.startsWith("---") ? "delete" : line.startsWith("@@") ? "hunk" : line.startsWith("diff ") ? "file-header" : "context"}><span>{index + 1}</span><code>{line || " "}</code>{line.startsWith("@@") && snapshotId && <button onClick={() => { const hunk = diff.hunks.find((item) => item.header === line); if (hunk) onRun({ type: diff.staged ? "unstageHunk" : "stageHunk", snapshotId, hunkId: hunk.id }); }}>{diff.staged ? t("unstageHunk") : t("stageHunk")}</button>}</div>)}</div></div>;
}

function HistoryCanvas({ repositoryId, onError }: { repositoryId: number; onError: (message: string) => void }) {
  const { t } = useI18n();
  const [commits, setCommits] = useState<CommitInfo[]>([]);
  useEffect(() => { api.history(repositoryId).then((page) => setCommits(page.commits)).catch((error) => onError(errorMessage(error))); }, [repositoryId, onError]);
  return <div className="history-canvas"><header className="canvas-header"><strong>{t("repositoryGraph")}</strong><span>{commits.length} {t("commitsLoaded")}</span></header><div className="graph-list">{commits.map((commit) => <div className="graph-row" key={commit.oid}><div className="graph-rail" style={{ "--lane": commit.lane.column } as React.CSSProperties}><i /></div><code>{shortOid(commit.oid)}</code><strong>{commit.subject}</strong><span>{commit.author}</span><time>{commit.authoredAt.slice(0, 10)}</time></div>)}</div></div>;
}

function HistoryPane({ repositoryId, onRun, onDiff, onError }: { repositoryId: number; onRun: (request: OperationRequest) => void; onDiff: (diff: string) => void; onError: (message: string) => void }) {
  const { t } = useI18n();
  const [commits, setCommits] = useState<CommitInfo[]>([]);
  useEffect(() => { api.history(repositoryId).then((page) => setCommits(page.commits)).catch((error) => onError(errorMessage(error))); }, [repositoryId, onError]);
  return <div><div className="pane-title"><span>{t("commits")}</span><code>{commits.length}</code></div><div className="object-list">{commits.map((commit) => <div className="object-action-row" key={commit.oid}><button onClick={() => api.commitDiff(repositoryId, commit.oid).then(onDiff).catch((error) => onError(errorMessage(error)))}><strong>{commit.subject}</strong><span>{commit.author} · {shortOid(commit.oid)}</span></button><RowMenu><button onClick={() => onRun({ type: "cherryPick", commits: [commit.oid] })}>{t("cherryPick")}</button>{commit.parents.length === 1 && <button onClick={() => onRun({ type: "revert", oid: commit.oid })}>{t("revert")}</button>}</RowMenu></div>)}</div></div>;
}

function BranchCanvas({ repository }: { repository?: RepositorySummary }) { const { t } = useI18n(); return <div className="canvas-empty"><div className="branch-hero"><RailMark /><code>{repository?.branch ?? t("detachedHead")}</code></div><h2>{t("refsIntegration")}</h2><p>{t("branchHint")}</p></div>; }
function StashCanvas({ repository }: { repository?: RepositorySummary }) { const { t } = useI18n(); return <div className="canvas-empty"><div className="change-tally"><strong>≋</strong><span>{t("savedStates")}</span></div><h2>{repository?.name} {t("stashes")}</h2><p>{t("stashHint")}</p></div>; }

function BranchesPane({ repositoryId, onRun, onDiff, onError }: { repositoryId: number; onRun: (request: OperationRequest) => void; onDiff: (diff: string) => void; onError: (message: string) => void }) {
  const { t } = useI18n();
  const [section, setSection] = useState<"branches" | "tags" | "remotes" | "submodules">("branches");
  const [creatingBranch, setCreatingBranch] = useState(false); const [branchName, setBranchName] = useState("");
  const [branches, setBranches] = useState<BranchInfo[]>([]); const [tags, setTags] = useState<TagInfo[]>([]); const [remotes, setRemotes] = useState<RemoteInfo[]>([]); const [submodules, setSubmodules] = useState<SubmoduleInfo[]>([]);
  useEffect(() => { Promise.all([api.branches(repositoryId), api.tags(repositoryId), api.remotes(repositoryId), api.submodules(repositoryId)]).then(([b, t, r, s]) => { setBranches(b); setTags(t); setRemotes(r); setSubmodules(s); }).catch((error) => onError(errorMessage(error))); }, [repositoryId, onError]);
  const createBranch = (event: React.FormEvent) => { event.preventDefault(); const name = branchName.trim(); if (!name) return; onRun({ type: "createBranch", name, checkout: true }); setBranchName(""); setCreatingBranch(false); };
  const compare = async () => { const base = window.prompt(t("baseBranch")); const head = base && window.prompt(t("headBranch")); if (base && head) api.compareBranches(repositoryId, base, head).then(onDiff).catch((error) => onError(errorMessage(error))); };
  return <div><div className="segmented">{(["branches", "tags", "remotes", "submodules"] as const).map((item) => <button className={section === item ? "active" : ""} key={item} onClick={() => setSection(item)}>{t(item)}</button>)}</div>{section === "branches" && <><div className="pane-title"><span>{t("branches")}</span><span><button onClick={compare}>{t("compare")}</button><button onClick={() => setCreatingBranch(true)}>{t("newBranch")}</button></span></div>{creatingBranch && <form className="new-branch-form" onSubmit={createBranch}><input autoFocus aria-label={t("newBranchName")} value={branchName} onChange={(event) => setBranchName(event.target.value)} onKeyDown={(event) => { if (event.key === "Escape") { setBranchName(""); setCreatingBranch(false); } }} /><button type="submit" disabled={!branchName.trim()}>{t("create")}</button><button type="button" onClick={() => { setBranchName(""); setCreatingBranch(false); }}>{t("cancel")}</button></form>}<div className="object-list">{branches.map((branch) => <div className="object-action-row" key={`${branch.remote}-${branch.name}`}><button className={branch.current ? "current" : ""} onDoubleClick={() => !branch.remote && !branch.current && onRun({ type: "switchBranch", name: branch.name })}><strong>{branch.current && "● "}{branch.name}</strong><span>{shortOid(branch.oid)} {branch.upstream && `· ${branch.upstream}`}</span></button><RowMenu>{branch.remote ? <button onClick={() => { const [remote, ...parts] = branch.name.split("/"); onRun({ type: "deleteRemoteBranch", remote, branch: parts.join("/") }); }}>{t("deleteRemoteBranch")}</button> : <>{!branch.current && <button onClick={() => onRun({ type: "switchBranch", name: branch.name })}>{t("switch")}</button>}{!branch.current && <button onClick={() => onRun({ type: "merge", reference: branch.name, mode: "normal" })}>{t("merge")}</button>}{!branch.current && <button onClick={() => onRun({ type: "merge", reference: branch.name, mode: "fastForward" })}>{t("fastForward")}</button>}{!branch.current && <button onClick={() => onRun({ type: "merge", reference: branch.name, mode: "squash" })}>{t("squashMerge")}</button>}{!branch.current && <button onClick={() => onRun({ type: "rebase", onto: branch.name })}>{t("rebaseOnto")}</button>}<button onClick={() => { const name = window.prompt(t("newBranchName"), branch.name); if (name) onRun({ type: "renameBranch", oldName: branch.name, newName: name }); }}>{t("rename")}</button>{!branch.current && <button onClick={() => onRun({ type: "deleteBranch", name: branch.name, force: false })}>{t("delete")}</button>}{!branch.current && <button onClick={() => onRun({ type: "deleteBranch", name: branch.name, force: true })}>{t("forceDelete")}</button>}</>}</RowMenu></div>)}</div></>}{section === "tags" && <><div className="pane-title"><span>{t("tags")}</span><button onClick={() => { const name = window.prompt(t("tagName")); const message = name && window.prompt(t("annotation")); if (name) onRun({ type: "createTag", name, message: message || undefined }); }}>＋</button></div><div className="object-list">{tags.map((tag) => <div className="object-action-row" key={tag.name}><button><strong>{tag.name}</strong><span>{tag.subject || shortOid(tag.oid)}</span></button><RowMenu><button onClick={() => { const remote = window.prompt(t("remote"), "origin"); if (remote) onRun({ type: "pushTag", remote, name: tag.name }); }}>{t("pushTag")}</button><button onClick={() => onRun({ type: "deleteLocalTag", name: tag.name })}>{t("deleteLocalTag")}</button></RowMenu></div>)}</div></>}{section === "remotes" && <><div className="pane-title"><span>{t("remotes")}</span><button onClick={() => { const name = window.prompt(t("remoteName")); const url = name && window.prompt(t("remoteUrl")); if (name && url) onRun({ type: "addRemote", name, url }); }}>＋</button></div><div className="object-list">{remotes.map((remote) => <div className="object-action-row" key={remote.name}><button><strong>{remote.name}</strong><span>{remote.fetchUrl}</span></button><RowMenu><button onClick={() => { const url = window.prompt(t("newRemoteUrl")); if (url) onRun({ type: "setRemoteUrl", name: remote.name, url }); }}>{t("editUrl")}</button><button onClick={() => onRun({ type: "removeRemote", name: remote.name })}>{t("removeRemote")}</button></RowMenu></div>)}</div></>}{section === "submodules" && <><div className="pane-title"><span>{t("submodules")}</span><span><button onClick={() => onRun({ type: "submoduleInit", paths: [], recursive: false })}>{t("init")}</button><button onClick={() => onRun({ type: "submoduleSync", paths: [], recursive: false })}>{t("sync")}</button><button onClick={() => onRun({ type: "submoduleUpdate", paths: [], recursive: window.confirm(t("updateNested")) })}>{t("update")}</button></span></div><div className="object-list">{submodules.map((module) => <button key={module.path}><strong>{module.path}</strong><span>{module.state} · {shortOid(module.oid)}</span></button>)}</div></>}</div>;
}

function StashesPane({ repositoryId, onRun, onError }: { repositoryId: number; onRun: (request: OperationRequest) => void; onError: (message: string) => void }) {
  const { t } = useI18n();
  const [stashes, setStashes] = useState<StashInfo[]>([]);
  useEffect(() => { api.stashes(repositoryId).then(setStashes).catch((error) => onError(errorMessage(error))); }, [repositoryId, onError]);
  const create = () => { const message = window.prompt(t("stashMessage")) || undefined; onRun({ type: "stashCreate", message, includeUntracked: window.confirm(t("includeUntracked")) }); };
  return <div><div className="pane-title"><span>{t("stashes")}</span><button onClick={create}>＋</button></div><div className="object-list">{stashes.map((stash) => <div className="stash-row" key={stash.oid}><button><strong>stash@{`{${stash.index}}`}</strong><span>{stash.subject}</span></button><div><button onClick={() => onRun({ type: "stashApply", index: stash.index, pop: false })}>{t("apply")}</button><button onClick={() => onRun({ type: "stashApply", index: stash.index, pop: true })}>{t("pop")}</button><button onClick={() => onRun({ type: "stashDrop", index: stash.index })}>{t("drop")}</button></div></div>)}</div></div>;
}

function ConfirmDialog({ pending, onCancel, onConfirm }: { pending: Pending; onCancel: () => void; onConfirm: () => void }) {
  const { t } = useI18n();
  return <div className="modal-backdrop" role="presentation"><section className={`confirm-dialog risk-${pending.preview.risk}`} role="alertdialog" aria-modal="true" aria-labelledby="confirm-title"><div className="risk-stripe" /><header><span>{pending.preview.risk === "destructive" ? t("irreversible") : t("reviewOperation")}</span><h2 id="confirm-title">{pending.preview.title}</h2></header><p>{pending.preview.summary}</p>{pending.preview.affectedPaths.length > 0 && <div className="impact"><label>{t("affectedPaths")}</label>{pending.preview.affectedPaths.map((path) => <code key={path}>{path}</code>)}</div>}{pending.preview.affectedRefs.length > 0 && <div className="impact"><label>{t("affectedRefs")}</label>{pending.preview.affectedRefs.map((ref) => <code key={ref}>{ref}</code>)}</div>}<footer><span>{pending.preview.recoverable ? t("recoverable") : t("unrecoverable")}</span><button onClick={onCancel}>{t("cancel")}</button><button className="danger" onClick={onConfirm}>{pending.preview.title}</button></footer></section></div>;
}
