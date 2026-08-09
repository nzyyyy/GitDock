import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import { api, type BranchInfo, type CommitInfo, type DiffFile, type FileChange, type GitInfo, type OperationEvent, type OperationPreview, type OperationRequest, type RemoteInfo, type RepositorySummary, type StashInfo, type SubmoduleInfo, type TagInfo, type WorkingTreeSnapshot } from "./api";

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
  const allowClose = useRef(false);

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
    }).catch((error) => { pushLog("error", errorMessage(error)); setOutputOpen(true); });
  }, [pushLog]);

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
      if (window.confirm(`${busyOperations.length} Git operation(s) are still running. Cancel them and quit?`)) {
        await Promise.allSettled(busyOperations.map(api.cancel));
        allowClose.current = true;
        await getCurrentWindow().close();
      }
    });
    return () => { listener.then((unlisten) => unlisten()); };
  }, [busyOperations]);

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
    const url = window.prompt("Remote URL"); if (!url) return;
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
    if (!selected || !window.confirm(`Remove ${selected.name} from GitDock? Files on disk are not changed.`)) return;
    await api.removeRepository(selected.id); setSelectedId(repositories.find((item) => item.id !== selected.id)?.id); await refreshRepositories();
  };
  const selectGit = async () => {
    const path = await open({ directory: false, multiple: false, title: "Select Git executable" });
    if (typeof path === "string") setGit(await api.setGitPath(path));
  };
  const forcePush = async () => {
    if (!selectedId || !selected?.branch) return;
    const remote = window.prompt("Remote", "origin"); if (!remote) return;
    try {
      const branches = await api.branches(selectedId);
      const expectedOid = branches.find((branch) => branch.remote && branch.name === `${remote}/${selected.branch}`)?.oid;
      if (!expectedOid) throw new Error(`Fetch ${remote}/${selected.branch} before force pushing.`);
      await run({ type: "forcePushWithLease", remote, branch: selected.branch, expectedOid });
    } catch (error) { pushLog("error", errorMessage(error)); setOutputOpen(true); }
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

  if (!repositories.length) return <EmptyState git={git} onAdd={register} onClone={clone} onInit={initialize} onSelectGit={selectGit} logs={logs} />;

  return (
    <div className="app-shell" style={{ gridTemplateColumns: `${leftWidth}px 1fr` }}>
      <aside className="repo-sidebar">
        <header className="brand"><RailMark /><div><strong>GitDock</strong><span>{git.supported ? `Git ${git.version}` : "Git unavailable"}</span></div></header>
        <label className="search"><span>⌕</span><input aria-label="Search repositories" placeholder="Find repository" value={filter} onChange={(event) => setFilter(event.target.value)} /></label>
        <div className="repo-list" role="listbox" aria-label="Repositories">
          {visibleRepositories.map((repository) => <RepositoryRow key={repository.id} repository={repository} selected={repository.id === selectedId} onSelect={() => setSelectedId(repository.id)} />)}
        </div>
        <footer className="sidebar-actions"><button onClick={register}>Add</button><button onClick={clone}>Clone</button><button onClick={initialize}>Init</button></footer><div className="resize-handle resize-left" onPointerDown={(event) => beginResize("left", event)} />
      </aside>

      <main className="workspace">
        <header className="topbar">
          <div className="repo-context"><span className="branch-dot" /> <strong>{selected?.name}</strong><code>{selected?.branch || shortOid(selected?.headOid)}</code></div>
          <nav aria-label="Workflows">{(["changes", "history", "branches", "stashes"] as Tab[]).map((item) => <button key={item} className={tab === item ? "active" : ""} onClick={() => { setTab(item); setDiff(undefined); }}>{item[0].toUpperCase() + item.slice(1)}</button>)}</nav>
          <div className="sync-actions"><button disabled={!selected?.capabilities.canManageRemotes} onClick={() => run({ type: "fetch", prune: false })}>Fetch</button><button disabled={!selected?.capabilities.canWriteWorkTree} onClick={() => run({ type: "pull" })}>Pull</button><button className="primary" disabled={!selected?.capabilities.canManageRemotes} onClick={() => run({ type: "push" })}>Push</button><RowMenu label="More"><button onClick={refreshRepositories}>Refresh all</button><button onClick={() => run({ type: "pull", strategy: "merge" })}>Pull with merge</button><button onClick={() => run({ type: "pull", strategy: "rebase" })}>Pull with rebase</button><button onClick={() => run({ type: "pull", strategy: "fastForwardOnly" })}>Pull fast-forward only</button><button onClick={forcePush}>Force push with lease</button><button onClick={() => { const remote = window.prompt("Remote", "origin"); const branch = remote && selected?.branch; if (remote && branch) run({ type: "setUpstream", remote, branch }); }}>Set upstream</button><button onClick={() => { const commits = window.prompt("Commit OIDs, separated by spaces")?.trim().split(/\s+/); if (commits?.length) run({ type: "cherryPick", commits }); }}>Cherry-pick commits</button><button onClick={() => run({ type: "undoLastCommit" })}>Undo last commit</button><button onClick={() => { const name = window.prompt("Repository name", selected?.name); if (name) updateSelected({ name }); }}>Rename entry</button><button onClick={() => updateSelected({ favorite: !selected?.favorite })}>{selected?.favorite ? "Remove favorite" : "Add favorite"}</button><button onClick={() => { const group = window.prompt("Group", selected?.group ?? ""); if (group !== null) updateSelected({ group }); }}>Set group</button><button onClick={relocateSelected}>Relocate</button><button onClick={selectGit}>Select Git executable</button><button className="menu-danger" onClick={removeSelected}>Remove from GitDock</button></RowMenu></div>
        </header>

        <div className="work-area" style={{ gridTemplateColumns: `minmax(480px, 1fr) ${rightWidth}px` }}>
          <section className="canvas">
            {diff ? <DiffView diff={diff} snapshotId={snapshot?.id} onBack={() => setDiff(undefined)} onRun={run} /> : tab === "changes" ? <ChangesOverview repository={selected} snapshot={snapshot} /> : tab === "history" ? <HistoryCanvas repositoryId={selectedId!} onError={(message) => { pushLog("error", message); setOutputOpen(true); }} /> : tab === "branches" ? <BranchCanvas repository={selected} /> : <StashCanvas repository={selected} />}
          </section>
          <aside className="tool-pane"><div className="resize-handle resize-right" onPointerDown={(event) => beginResize("right", event)} />
            {tab === "changes" && <ChangesPane repository={selected} snapshot={snapshot} onOpen={openDiff} onOpenExternal={(path) => api.openRepositoryFile(selectedId!, path).catch((error) => pushLog("error", errorMessage(error)))} onLoadIgnored={() => refreshStatus(selectedId, true)} onRun={run} />}
            {tab === "history" && <HistoryPane repositoryId={selectedId!} onRun={run} onDiff={(value) => setDiff({ path: "Commit", staged: false, binary: false, tooLarge: false, patch: value, hunks: [] })} onError={(message) => { pushLog("error", message); setOutputOpen(true); }} />}
            {tab === "branches" && <BranchesPane repositoryId={selectedId!} onRun={run} onDiff={(value) => setDiff({ path: "Branch comparison", staged: false, binary: false, tooLarge: false, patch: value, hunks: [] })} onError={(message) => pushLog("error", message)} />}
            {tab === "stashes" && <StashesPane repositoryId={selectedId!} onRun={run} onError={(message) => pushLog("error", message)} />}
          </aside>
        </div>

        <section className={`output-panel ${outputOpen ? "open" : ""}`}>
          <button className="output-handle" onClick={() => setOutputOpen((value) => !value)}><span>Git output</span><span>{busyOperations.length ? `${busyOperations.length} running` : `${logs.length} lines`} {outputOpen ? "⌄" : "⌃"}</span></button>
          {outputOpen && <><div className="resize-handle resize-output" onPointerDown={(event) => beginResize("output", event)} /><div className="log" style={{ height: outputHeight }}><div className="log-toolbar">{busyOperations.map((id) => <button key={id} onClick={() => api.cancel(id)}>Cancel #{id}</button>)}<button onClick={() => setLogs([])}>Clear</button></div>{logs.map((line) => <div key={line.id} className={`log-${line.kind}`}>{line.message}</div>)}</div></>}
        </section>
      </main>

      {pending && <ConfirmDialog pending={pending} onCancel={() => setPending(undefined)} onConfirm={confirmPending} />}
    </div>
  );
}

function EmptyState({ git, onAdd, onClone, onInit, onSelectGit, logs }: { git: GitInfo; onAdd: () => void; onClone: () => void; onInit: () => void; onSelectGit: () => void; logs: LogLine[] }) {
  return <main className="empty-state"><div className="empty-brand"><RailMark /><span>GITDOCK / WORKSPACE</span></div><h1>Put every working tree<br />on one rail.</h1><p>Inspect changes, shape commits, and move between branches without losing the state of another repository.</p><div className="empty-actions"><button className="primary" disabled={!git.supported} onClick={onAdd}>Add repository</button><button disabled={!git.supported} onClick={onClone}>Clone</button><button disabled={!git.supported} onClick={onInit}>Initialize</button>{!git.supported && <button onClick={onSelectGit}>Select Git executable</button>}</div><div className={`git-check ${git.supported ? "ok" : "bad"}`}><span>{git.supported ? "●" : "×"}</span><div><strong>{git.supported ? `Git ${git.version}` : "Git 2.30+ required"}</strong><small>{git.path ?? git.error}</small></div></div>{logs.at(-1) && <p className="empty-error">{logs.at(-1)?.message}</p>}</main>;
}

function RailMark() { return <svg className="rail-mark" viewBox="0 0 32 32" aria-hidden="true"><path d="M9 4v18a6 6 0 0 0 6 6h3" /><path d="M23 4v7a5 5 0 0 1-5 5H9" /><circle cx="9" cy="4" r="2.5" /><circle cx="23" cy="4" r="2.5" /><circle cx="20" cy="28" r="2.5" /></svg>; }

function RepositoryRow({ repository, selected, onSelect }: { repository: RepositorySummary; selected: boolean; onSelect: () => void }) {
  const state = repository.kind === "missing" ? "missing" : repository.conflictCount ? "conflict" : repository.changedCount ? "changed" : "clean";
  return <button role="option" aria-selected={selected} className={`repo-row ${selected ? "selected" : ""}`} onClick={onSelect}><span className={`status-rail ${state}`} /><span className="repo-copy"><span className="repo-name">{repository.favorite && "★ "}{repository.name}<i>{repository.conflictCount ? `${repository.conflictCount} conflicts` : state}</i></span><span className="repo-meta"><code>{repository.branch || shortOid(repository.headOid)}</code><span>{repository.changedCount ? `±${repository.changedCount}` : "clean"}</span>{(repository.ahead || repository.behind) ? <span>↑{repository.ahead} ↓{repository.behind}</span> : null}</span></span></button>;
}

function ChangesOverview({ repository, snapshot }: { repository?: RepositorySummary; snapshot?: WorkingTreeSnapshot }) {
  const changed = snapshot?.files.filter((file) => !file.ignored).length ?? 0;
  if (!changed) return <div className="canvas-empty"><span className="large-check">✓</span><h2>Working tree clean</h2><p>{repository?.lastCommit || "No local changes"}</p></div>;
  return <div className="canvas-empty"><div className="change-tally"><strong>{changed}</strong><span>working tree<br />changes</span></div><h2>Select a file to inspect</h2><p>The center canvas shows the exact patch GitDock will stage or unstage.</p></div>;
}

function ChangesPane({ repository, snapshot, onOpen, onOpenExternal, onLoadIgnored, onRun }: { repository?: RepositorySummary; snapshot?: WorkingTreeSnapshot; onOpen: (file: FileChange, staged: boolean) => void; onOpenExternal: (path: string) => void; onLoadIgnored: () => void; onRun: (request: OperationRequest) => void }) {
  const [message, setMessage] = useState(""); const [amend, setAmend] = useState(false); const [signoff, setSignoff] = useState(false);
  const files = snapshot?.files ?? [];
  const groups = [
    ["Conflicts", files.filter((f) => f.conflict), "conflict"],
    ["Staged", files.filter((f) => f.staged && !f.conflict), "staged"],
    ["Unstaged", files.filter((f) => f.unstaged && !f.conflict && f.kind !== "Untracked" && !f.ignored), "unstaged"],
    ["Untracked", files.filter((f) => f.kind === "Untracked"), "untracked"],
    ["Ignored", files.filter((f) => f.ignored), "ignored"],
  ] as const;
  return <div className="changes-pane"><div className="pane-title"><span>Working tree</span><button onClick={onLoadIgnored}>Load ignored</button></div><div className="change-groups">{repository?.ongoing && <div className="ongoing"><strong>{repository.ongoing.kind} in progress</strong>{repository.ongoing.canContinue && <button onClick={() => onRun({ type: "continue", kind: repository.ongoing!.kind })}>Continue</button>}{repository.ongoing.canSkip && <button onClick={() => onRun({ type: "skip", kind: repository.ongoing!.kind })}>Skip</button>}{repository.ongoing.canAbort && <button onClick={() => onRun({ type: "abort", kind: repository.ongoing!.kind })}>Abort</button>}</div>}{groups.map(([name, entries, type]) => <ChangeGroup key={name} name={name} files={entries} type={type} onOpen={onOpen} onOpenExternal={onOpenExternal} onRun={onRun} />)}</div><form className="commit-box" onSubmit={(event) => { event.preventDefault(); onRun({ type: "commit", message, amend, signoff }); }}><label>Commit message<textarea value={message} onChange={(event) => setMessage(event.target.value)} placeholder="Summarize the change" /></label><div className="commit-options"><label><input type="checkbox" checked={amend} onChange={(event) => setAmend(event.target.checked)} /> Amend</label><label><input type="checkbox" checked={signoff} onChange={(event) => setSignoff(event.target.checked)} /> Sign off</label></div><button className="primary" disabled={!message.trim()}>Commit staged changes</button></form></div>;
}

function RowMenu({ children, label = "More actions" }: { children: React.ReactNode; label?: string }) {
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
  return <><button ref={buttonRef} className="row-menu-trigger" type="button" aria-label={label} aria-haspopup="menu" aria-expanded={open} onClick={toggle}>{label === "More actions" ? "•••" : label}</button><div ref={menuRef} className="row-menu-popover" popover="auto" role="menu" onToggle={(event) => setOpen(event.newState === "open")} onClick={(event) => { if ((event.target as HTMLElement).closest("button")) menuRef.current?.hidePopover(); }}>{children}</div></>;
}

function ChangeGroup({ name, files, type, onOpen, onOpenExternal, onRun }: { name: string; files: FileChange[]; type: string; onOpen: (file: FileChange, staged: boolean) => void; onOpenExternal: (path: string) => void; onRun: (request: OperationRequest) => void }) {
  if (!files.length) return null;
  return <section className="change-group"><header><span>{name}</span><code>{files.length}</code></header>{files.map((file) => <div className={`file-row ${type === "conflict" ? "conflict-row" : ""}`} key={`${type}-${file.path}`}><button className="file-main" onClick={() => onOpen(file, type === "staged")}><b>{file.path.split("/").at(-1)}</b><small>{file.path.includes("/") ? file.path.slice(0, file.path.lastIndexOf("/")) : "./"}</small></button><span className={`file-kind kind-${file.kind.toLowerCase()}`}>{file.kind[0]}</span>{type === "staged" ? <button onClick={() => onRun({ type: "unstageFiles", paths: [file.path] })}>Unstage</button> : type === "untracked" ? <><button onClick={() => onRun({ type: "stageFiles", paths: [file.path] })}>Stage</button><button className="danger-icon" aria-label={`Trash ${file.path}`} onClick={() => onRun({ type: "trashUntracked", paths: [file.path] })}>⌫</button></> : type === "conflict" ? <RowMenu label="Resolve"><button onClick={() => onRun({ type: "chooseConflictSide", path: file.path, side: "ours" })}>Use current target</button><button onClick={() => onRun({ type: "chooseConflictSide", path: file.path, side: "theirs" })}>Use incoming commit</button><button onClick={() => onOpenExternal(file.path)}>Open externally</button><button onClick={() => onRun({ type: "runMergetool", path: file.path })}>Run configured mergetool</button><button onClick={() => onRun({ type: "markResolved", paths: [file.path] })}>Mark resolved</button></RowMenu> : type === "ignored" ? null : <><button onClick={() => onRun({ type: "stageFiles", paths: [file.path] })}>Stage</button><button className="danger-icon" aria-label={`Discard ${file.path}`} onClick={() => onRun({ type: "discardTracked", paths: [file.path] })}>↶</button></>}</div>)}</section>;
}

function DiffView({ diff, snapshotId, onBack, onRun }: { diff: DiffFile; snapshotId?: number; onBack: () => void; onRun: (request: OperationRequest) => void }) {
  if (diff.binary || diff.tooLarge) return <div className="diff-view"><header className="canvas-header"><button onClick={onBack}>← Back</button><strong>{diff.path}</strong></header><div className="canvas-empty"><h2>{diff.binary ? "Binary diff" : "Diff exceeds the safe preview limit"}</h2><button onClick={() => onRun({ type: "runDifftool", path: diff.path, staged: diff.staged })}>Open configured difftool</button></div></div>;
  const lines = diff.patch.split("\n");
  return <div className="diff-view"><header className="canvas-header"><button onClick={onBack}>← Back</button><strong>{diff.path}</strong><span>{diff.staged ? "INDEX ↔ HEAD" : "WORKTREE ↔ INDEX"}</span></header><div className="diff-lines">{lines.map((line, index) => <div key={index} className={line.startsWith("+") && !line.startsWith("+++") ? "add" : line.startsWith("-") && !line.startsWith("---") ? "delete" : line.startsWith("@@") ? "hunk" : line.startsWith("diff ") ? "file-header" : "context"}><span>{index + 1}</span><code>{line || " "}</code>{line.startsWith("@@") && snapshotId && <button onClick={() => { const hunk = diff.hunks.find((item) => item.header === line); if (hunk) onRun({ type: diff.staged ? "unstageHunk" : "stageHunk", snapshotId, hunkId: hunk.id }); }}>{diff.staged ? "Unstage hunk" : "Stage hunk"}</button>}</div>)}</div></div>;
}

function HistoryCanvas({ repositoryId, onError }: { repositoryId: number; onError: (message: string) => void }) {
  const [commits, setCommits] = useState<CommitInfo[]>([]);
  useEffect(() => { api.history(repositoryId).then((page) => setCommits(page.commits)).catch((error) => onError(errorMessage(error))); }, [repositoryId, onError]);
  return <div className="history-canvas"><header className="canvas-header"><strong>Repository graph</strong><span>{commits.length} commits loaded</span></header><div className="graph-list">{commits.map((commit) => <div className="graph-row" key={commit.oid}><div className="graph-rail" style={{ "--lane": commit.lane.column } as React.CSSProperties}><i /></div><code>{shortOid(commit.oid)}</code><strong>{commit.subject}</strong><span>{commit.author}</span><time>{commit.authoredAt.slice(0, 10)}</time></div>)}</div></div>;
}

function HistoryPane({ repositoryId, onRun, onDiff, onError }: { repositoryId: number; onRun: (request: OperationRequest) => void; onDiff: (diff: string) => void; onError: (message: string) => void }) {
  const [commits, setCommits] = useState<CommitInfo[]>([]);
  useEffect(() => { api.history(repositoryId).then((page) => setCommits(page.commits)).catch((error) => onError(errorMessage(error))); }, [repositoryId, onError]);
  return <div><div className="pane-title"><span>Commits</span><code>{commits.length}</code></div><div className="object-list">{commits.map((commit) => <div className="object-action-row" key={commit.oid}><button onClick={() => api.commitDiff(repositoryId, commit.oid).then(onDiff).catch((error) => onError(errorMessage(error)))}><strong>{commit.subject}</strong><span>{commit.author} · {shortOid(commit.oid)}</span></button><RowMenu><button onClick={() => onRun({ type: "cherryPick", commits: [commit.oid] })}>Cherry-pick</button>{commit.parents.length === 1 && <button onClick={() => onRun({ type: "revert", oid: commit.oid })}>Revert</button>}</RowMenu></div>)}</div></div>;
}

function BranchCanvas({ repository }: { repository?: RepositorySummary }) { return <div className="canvas-empty"><div className="branch-hero"><RailMark /><code>{repository?.branch ?? "detached HEAD"}</code></div><h2>Refs and integration</h2><p>Choose a branch, tag, remote, or submodule from the right pane.</p></div>; }
function StashCanvas({ repository }: { repository?: RepositorySummary }) { return <div className="canvas-empty"><div className="change-tally"><strong>≋</strong><span>saved working<br />tree states</span></div><h2>{repository?.name} stashes</h2><p>Apply, pop, or inspect a saved worktree state.</p></div>; }

function BranchesPane({ repositoryId, onRun, onDiff, onError }: { repositoryId: number; onRun: (request: OperationRequest) => void; onDiff: (diff: string) => void; onError: (message: string) => void }) {
  const [section, setSection] = useState<"branches" | "tags" | "remotes" | "submodules">("branches");
  const [creatingBranch, setCreatingBranch] = useState(false); const [branchName, setBranchName] = useState("");
  const [branches, setBranches] = useState<BranchInfo[]>([]); const [tags, setTags] = useState<TagInfo[]>([]); const [remotes, setRemotes] = useState<RemoteInfo[]>([]); const [submodules, setSubmodules] = useState<SubmoduleInfo[]>([]);
  useEffect(() => { Promise.all([api.branches(repositoryId), api.tags(repositoryId), api.remotes(repositoryId), api.submodules(repositoryId)]).then(([b, t, r, s]) => { setBranches(b); setTags(t); setRemotes(r); setSubmodules(s); }).catch((error) => onError(errorMessage(error))); }, [repositoryId, onError]);
  const createBranch = (event: React.FormEvent) => { event.preventDefault(); const name = branchName.trim(); if (!name) return; onRun({ type: "createBranch", name, checkout: true }); setBranchName(""); setCreatingBranch(false); };
  const compare = async () => { const base = window.prompt("Base branch"); const head = base && window.prompt("Head branch"); if (base && head) api.compareBranches(repositoryId, base, head).then(onDiff).catch((error) => onError(errorMessage(error))); };
  return <div><div className="segmented">{(["branches", "tags", "remotes", "submodules"] as const).map((item) => <button className={section === item ? "active" : ""} key={item} onClick={() => setSection(item)}>{item}</button>)}</div>{section === "branches" && <><div className="pane-title"><span>Branches</span><span><button onClick={compare}>Compare</button><button onClick={() => setCreatingBranch(true)}>New branch</button></span></div>{creatingBranch && <form className="new-branch-form" onSubmit={createBranch}><input autoFocus aria-label="New branch name" value={branchName} onChange={(event) => setBranchName(event.target.value)} onKeyDown={(event) => { if (event.key === "Escape") { setBranchName(""); setCreatingBranch(false); } }} /><button type="submit" disabled={!branchName.trim()}>Create</button><button type="button" onClick={() => { setBranchName(""); setCreatingBranch(false); }}>Cancel</button></form>}<div className="object-list">{branches.map((branch) => <div className="object-action-row" key={`${branch.remote}-${branch.name}`}><button className={branch.current ? "current" : ""} onDoubleClick={() => !branch.remote && !branch.current && onRun({ type: "switchBranch", name: branch.name })}><strong>{branch.current && "● "}{branch.name}</strong><span>{shortOid(branch.oid)} {branch.upstream && `· ${branch.upstream}`}</span></button><RowMenu>{branch.remote ? <button onClick={() => { const [remote, ...parts] = branch.name.split("/"); onRun({ type: "deleteRemoteBranch", remote, branch: parts.join("/") }); }}>Delete remote branch</button> : <>{!branch.current && <button onClick={() => onRun({ type: "switchBranch", name: branch.name })}>Switch</button>}{!branch.current && <button onClick={() => onRun({ type: "merge", reference: branch.name, mode: "normal" })}>Merge</button>}{!branch.current && <button onClick={() => onRun({ type: "merge", reference: branch.name, mode: "fastForward" })}>Fast-forward only</button>}{!branch.current && <button onClick={() => onRun({ type: "merge", reference: branch.name, mode: "squash" })}>Squash merge</button>}{!branch.current && <button onClick={() => onRun({ type: "rebase", onto: branch.name })}>Rebase onto</button>}<button onClick={() => { const name = window.prompt("New branch name", branch.name); if (name) onRun({ type: "renameBranch", oldName: branch.name, newName: name }); }}>Rename</button>{!branch.current && <button onClick={() => onRun({ type: "deleteBranch", name: branch.name, force: false })}>Delete</button>}{!branch.current && <button onClick={() => onRun({ type: "deleteBranch", name: branch.name, force: true })}>Force delete</button>}</>}</RowMenu></div>)}</div></>}{section === "tags" && <><div className="pane-title"><span>Tags</span><button onClick={() => { const name = window.prompt("Tag name"); const message = name && window.prompt("Annotation (leave empty for lightweight tag)"); if (name) onRun({ type: "createTag", name, message: message || undefined }); }}>＋</button></div><div className="object-list">{tags.map((tag) => <div className="object-action-row" key={tag.name}><button><strong>{tag.name}</strong><span>{tag.subject || shortOid(tag.oid)}</span></button><RowMenu><button onClick={() => { const remote = window.prompt("Remote", "origin"); if (remote) onRun({ type: "pushTag", remote, name: tag.name }); }}>Push tag</button><button onClick={() => onRun({ type: "deleteLocalTag", name: tag.name })}>Delete local tag</button></RowMenu></div>)}</div></>}{section === "remotes" && <><div className="pane-title"><span>Remotes</span><button onClick={() => { const name = window.prompt("Remote name"); const url = name && window.prompt("Remote URL"); if (name && url) onRun({ type: "addRemote", name, url }); }}>＋</button></div><div className="object-list">{remotes.map((remote) => <div className="object-action-row" key={remote.name}><button><strong>{remote.name}</strong><span>{remote.fetchUrl}</span></button><RowMenu><button onClick={() => { const url = window.prompt("New remote URL"); if (url) onRun({ type: "setRemoteUrl", name: remote.name, url }); }}>Edit URL</button><button onClick={() => onRun({ type: "removeRemote", name: remote.name })}>Remove remote</button></RowMenu></div>)}</div></>}{section === "submodules" && <><div className="pane-title"><span>Submodules</span><span><button onClick={() => onRun({ type: "submoduleInit", paths: [], recursive: false })}>Init</button><button onClick={() => onRun({ type: "submoduleSync", paths: [], recursive: false })}>Sync</button><button onClick={() => onRun({ type: "submoduleUpdate", paths: [], recursive: window.confirm("Update nested submodules recursively?") })}>Update</button></span></div><div className="object-list">{submodules.map((module) => <button key={module.path}><strong>{module.path}</strong><span>{module.state} · {shortOid(module.oid)}</span></button>)}</div></>}</div>;
}

function StashesPane({ repositoryId, onRun, onError }: { repositoryId: number; onRun: (request: OperationRequest) => void; onError: (message: string) => void }) {
  const [stashes, setStashes] = useState<StashInfo[]>([]);
  useEffect(() => { api.stashes(repositoryId).then(setStashes).catch((error) => onError(errorMessage(error))); }, [repositoryId, onError]);
  const create = () => { const message = window.prompt("Stash message (optional)") || undefined; onRun({ type: "stashCreate", message, includeUntracked: window.confirm("Include untracked files?") }); };
  return <div><div className="pane-title"><span>Stashes</span><button onClick={create}>＋</button></div><div className="object-list">{stashes.map((stash) => <div className="stash-row" key={stash.oid}><button><strong>stash@{`{${stash.index}}`}</strong><span>{stash.subject}</span></button><div><button onClick={() => onRun({ type: "stashApply", index: stash.index, pop: false })}>Apply</button><button onClick={() => onRun({ type: "stashApply", index: stash.index, pop: true })}>Pop</button><button onClick={() => onRun({ type: "stashDrop", index: stash.index })}>Drop</button></div></div>)}</div></div>;
}

function ConfirmDialog({ pending, onCancel, onConfirm }: { pending: Pending; onCancel: () => void; onConfirm: () => void }) {
  return <div className="modal-backdrop" role="presentation"><section className={`confirm-dialog risk-${pending.preview.risk}`} role="alertdialog" aria-modal="true" aria-labelledby="confirm-title"><div className="risk-stripe" /><header><span>{pending.preview.risk === "destructive" ? "IRREVERSIBLE CHANGE" : "REVIEW OPERATION"}</span><h2 id="confirm-title">{pending.preview.title}</h2></header><p>{pending.preview.summary}</p>{pending.preview.affectedPaths.length > 0 && <div className="impact"><label>Affected paths</label>{pending.preview.affectedPaths.map((path) => <code key={path}>{path}</code>)}</div>}{pending.preview.affectedRefs.length > 0 && <div className="impact"><label>Affected refs</label>{pending.preview.affectedRefs.map((ref) => <code key={ref}>{ref}</code>)}</div>}<footer><span>{pending.preview.recoverable ? "Git can usually recover this change." : "GitDock cannot recover this change."}</span><button onClick={onCancel}>Cancel</button><button className="danger" onClick={onConfirm}>{pending.preview.title}</button></footer></section></div>;
}
