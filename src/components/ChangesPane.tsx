import { memo, useEffect, useRef, useState } from "react";
import type { FileChange, RepositorySummary, WorkingTreeSnapshot } from "../api";
import { useI18n } from "../i18n";
import type { RunOperation } from "../types";
import { RowMenu } from "./RepositoryPane";

export function ChangesOverview({ repository, snapshot }: { repository?: RepositorySummary; snapshot?: WorkingTreeSnapshot }) {
  const { t } = useI18n();
  const changed = snapshot?.files.filter((file) => !file.ignored).length ?? 0;
  if (!changed) return <div className="canvas-empty"><span className="large-check">✓</span><h2>{t("workingTreeClean")}</h2><p>{repository?.lastCommit || t("noLocalChanges")}</p></div>;
  return <div className="canvas-empty"><div className="change-tally"><strong>{changed}</strong><span>{t("workingTreeChanges")}</span></div><h2>{t("selectFile")}</h2><p>{t("inspectHint")}</p></div>;
}

export function ChangesPane({ repository, snapshot, onOpen, onOpenExternal, onLoadIgnored, onRun, onFileHistory, onBlame }: { repository?: RepositorySummary; snapshot?: WorkingTreeSnapshot; onOpen: (file: FileChange, staged: boolean) => void; onOpenExternal: (path: string) => void; onLoadIgnored: () => void; onRun: RunOperation; onFileHistory: (path: string) => void; onBlame: (path: string) => void }) {
  const { t } = useI18n();
  const [stageSelection, setStageSelection] = useState<string[]>([]); const [unstageSelection, setUnstageSelection] = useState<string[]>([]);
  const repositoryId = repository?.id;
  const files = snapshot?.files ?? [];
  const ignoredVisible = files.some((file) => file.ignored);
  const hasStagedChanges = files.some((file) => file.staged && !file.conflict);
  const groups = [
    [t("conflictsGroup"), files.filter((f) => f.conflict), "conflict"],
    [t("staged"), files.filter((f) => f.staged && !f.unstaged && !f.conflict), "staged"],
    [t("mixed"), files.filter((f) => f.staged && f.unstaged && !f.conflict), "mixed"],
    [t("unstaged"), files.filter((f) => f.unstaged && !f.staged && !f.conflict && f.kind !== "untracked" && !f.ignored), "unstaged"],
    [t("untracked"), files.filter((f) => f.kind === "untracked"), "untracked"],
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
  return <div className="changes-pane"><div className="pane-title"><span>{t("workingTree")}</span><span className="batch-actions">{stageSelection.length > 0 && <button onClick={() => batch("stageFiles", stageSelection, () => setStageSelection([]))}>{t("stageSelected")} ({stageSelection.length})</button>}{unstageSelection.length > 0 && <button onClick={() => batch("unstageFiles", unstageSelection, () => setUnstageSelection([]))}>{t("unstageSelected")} ({unstageSelection.length})</button>}{stageSelection.length === 0 && unstageSelection.length === 0 && <button onClick={onLoadIgnored}>{t(ignoredVisible ? "hideIgnored" : "loadIgnored")}</button>}</span></div><div className="change-groups">{repository?.ongoing && <div className="ongoing"><strong>{repository.ongoing.kind} {t("inProgress")}</strong>{repository.ongoing.canContinue && <button onClick={() => onRun({ type: "continue", kind: repository.ongoing!.kind })}>{t("continue")}</button>}{repository.ongoing.canSkip && <button onClick={() => onRun({ type: "skip", kind: repository.ongoing!.kind })}>{t("skip")}</button>}{repository.ongoing.canAbort && <button onClick={() => onRun({ type: "abort", kind: repository.ongoing!.kind })}>{t("abort")}</button>}</div>}{groups.map(([name, entries, type]) => { const selected = type === "staged" ? unstageSelection : stageSelection; const setSelected = type === "staged" ? setUnstageSelection : setStageSelection; return <ChangeGroup key={type} name={name} files={entries} type={type} selected={selected} onToggle={(path) => toggle(path, selected, setSelected)} onSelectAll={() => setSelected(entries.every((file) => selected.includes(file.path)) ? selected.filter((path) => !entries.some((file) => file.path === path)) : [...new Set([...selected, ...entries.map((file) => file.path)])])} onOpen={onOpen} onOpenExternal={onOpenExternal} onRun={onRun} onFileHistory={onFileHistory} onBlame={onBlame} />; })}</div><MemoCommitBox repositoryId={repositoryId} hasStagedChanges={hasStagedChanges} onRun={onRun} /></div>;
}

function CommitBox({ repositoryId, hasStagedChanges, onRun }: { repositoryId?: number; hasStagedChanges: boolean; onRun: RunOperation }) {
  const { t } = useI18n();
  const messages = useRef<Record<number, string>>({}); const textarea = useRef<HTMLTextAreaElement>(null); const activeRepositoryId = useRef(repositoryId); activeRepositoryId.current = repositoryId;
  const [nonemptyRepositories, setNonemptyRepositories] = useState<Set<number>>(() => new Set()); const [amend, setAmend] = useState(false); const [signoff, setSignoff] = useState(false);
  const [committingRepositories, setCommittingRepositories] = useState<Set<number>>(() => new Set());
  const commitRunning = repositoryId ? committingRepositories.has(repositoryId) : false;
  const commit = (event: React.FormEvent) => {
    event.preventDefault();
    if (!repositoryId) return;
    const submittedMessage = textarea.current?.value ?? messages.current[repositoryId] ?? "";
    const submittedRepositoryId = repositoryId;
    messages.current[submittedRepositoryId] = submittedMessage;
    setCommittingRepositories((current) => new Set(current).add(submittedRepositoryId));
    onRun({ type: "commit", message: submittedMessage, amend, signoff }, (outcome) => {
      setCommittingRepositories((current) => { const next = new Set(current); next.delete(submittedRepositoryId); return next; });
      if (outcome === "succeeded" && messages.current[submittedRepositoryId] === submittedMessage) {
        messages.current[submittedRepositoryId] = "";
        setNonemptyRepositories((current) => { if (!current.has(submittedRepositoryId)) return current; const next = new Set(current); next.delete(submittedRepositoryId); return next; });
        if (activeRepositoryId.current === submittedRepositoryId && textarea.current) textarea.current.value = "";
      }
    });
  };
  return <form className="commit-box" onSubmit={commit}><label>{t("commitMessage")}<textarea key={repositoryId} ref={textarea} name="commitMessage" autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} defaultValue={repositoryId ? messages.current[repositoryId] ?? "" : ""} onChange={(event) => { if (!repositoryId) return; const value = event.currentTarget.value; const wasNonempty = Boolean(messages.current[repositoryId]?.trim()); const isNonempty = Boolean(value.trim()); messages.current[repositoryId] = value; if (wasNonempty !== isNonempty) setNonemptyRepositories((current) => { const next = new Set(current); if (isNonempty) next.add(repositoryId); else next.delete(repositoryId); return next; }); }} placeholder={t("commitPlaceholder")} /></label><div className="commit-options"><label><input name="amend" type="checkbox" checked={amend} onChange={(event) => setAmend(event.target.checked)} /> {t("amend")}</label><label><input name="signoff" type="checkbox" checked={signoff} onChange={(event) => setSignoff(event.target.checked)} /> {t("signOff")}</label></div><button className="primary" disabled={commitRunning || !repositoryId || !nonemptyRepositories.has(repositoryId) || (!hasStagedChanges && !amend)}>{commitRunning ? `${t("running")}…` : t("commitStaged")}</button></form>;
}

function ChangeGroup({ name, files, type, selected, onToggle, onSelectAll, onOpen, onOpenExternal, onRun, onFileHistory, onBlame }: { name: string; files: FileChange[]; type: string; selected: string[]; onToggle: (path: string) => void; onSelectAll: () => void; onOpen: (file: FileChange, staged: boolean) => void; onOpenExternal: (path: string) => void; onRun: RunOperation; onFileHistory: (path: string) => void; onBlame: (path: string) => void }) {
  const { t } = useI18n();
  const [collapsed, setCollapsed] = useState(false);
  if (!files.length) return null;
  const selectable = type === "staged" || type === "unstaged" || type === "untracked";
  return <section className="change-group"><header>{selectable && <input type="checkbox" aria-label={`${t("selectAll")} ${name}`} checked={files.every((file) => selected.includes(file.path))} onChange={onSelectAll} />}<button type="button" aria-expanded={!collapsed} onClick={() => setCollapsed((current) => !current)}><span className="group-label">{name}<span className="group-chevron" aria-hidden="true">{collapsed ? "▸" : "▾"}</span></span><code>{files.length}</code></button></header>{!collapsed && files.map((file) => <div className={`file-row ${selectable ? "selectable" : ""} ${type === "conflict" ? "conflict-row" : ""}`} key={`${type}-${file.path}`}>{selectable && <input type="checkbox" aria-label={`${type === "staged" ? t("selectFileForUnstage") : t("selectFileForStage")} ${file.path}`} checked={selected.includes(file.path)} onChange={() => onToggle(file.path)} />}<button className="file-main" onClick={() => onOpen(file, type === "staged")}><b>{file.path.split("/").at(-1)}</b><small>{file.path.includes("/") ? file.path.slice(0, file.path.lastIndexOf("/")) : "./"}</small></button><span className={`file-kind kind-${file.kind.toLowerCase()}`}>{type === "untracked" ? "" : file.kind[0]}</span>{type === "staged" ? <><button className="stage-icon" aria-label={`${t("unstage")} ${file.path}`} onClick={() => onRun({ type: "unstageFiles", paths: [file.path] })}>−</button><RowMenu context><button onClick={() => onFileHistory(file.path)}>{t("fileHistory")}</button><button onClick={() => onBlame(file.path)}>{t("blame")}</button></RowMenu></> : type === "mixed" ? <><button className="stage-icon" aria-label={`${t("stage")} ${file.path}`} onClick={() => onRun({ type: "stageFiles", paths: [file.path] })}>＋</button><button className="stage-icon" aria-label={`${t("unstage")} ${file.path}`} onClick={() => onRun({ type: "unstageFiles", paths: [file.path] })}>−</button><RowMenu context><button onClick={() => onFileHistory(file.path)}>{t("fileHistory")}</button><button onClick={() => onBlame(file.path)}>{t("blame")}</button></RowMenu></> : type === "untracked" ? <><button className="stage-icon" aria-label={`${t("stage")} ${file.path}`} onClick={() => onRun({ type: "stageFiles", paths: [file.path] })}>＋</button><button className="danger-icon" aria-label={`${t("trash")} ${file.path}`} onClick={() => onRun({ type: "trashUntracked", paths: [file.path] })}>⌫</button></> : type === "conflict" ? <RowMenu context label={t("resolve")}><button onClick={() => onOpen(file, false)}>{t("openInternalEditor")}</button><button onClick={() => onRun({ type: "chooseConflictSide", path: file.path, side: "ours" })}>{t("useCurrent")}</button><button onClick={() => onRun({ type: "chooseConflictSide", path: file.path, side: "theirs" })}>{t("useIncoming")}</button><button onClick={() => onOpenExternal(file.path)}>{t("openExternal")}</button><button onClick={() => onRun({ type: "runMergetool", path: file.path })}>{t("runMergetool")}</button><button onClick={() => onRun({ type: "markResolved", paths: [file.path] })}>{t("markResolved")}</button><button onClick={() => onFileHistory(file.path)}>{t("fileHistory")}</button><button onClick={() => onBlame(file.path)}>{t("blame")}</button></RowMenu> : type === "ignored" ? null : <><button className="stage-icon" aria-label={`${t("stage")} ${file.path}`} onClick={() => onRun({ type: "stageFiles", paths: [file.path] })}>＋</button><button className="danger-icon" aria-label={`${t("discard")} ${file.path}`} onClick={() => onRun({ type: "discardTracked", paths: [file.path] })}>↶</button><RowMenu context><button onClick={() => onFileHistory(file.path)}>{t("fileHistory")}</button><button onClick={() => onBlame(file.path)}>{t("blame")}</button></RowMenu></>}</div>)}</section>;
}

export const MemoChangesOverview = memo(ChangesOverview);
export const MemoChangesPane = memo(ChangesPane);
export const MemoCommitBox = memo(CommitBox);
