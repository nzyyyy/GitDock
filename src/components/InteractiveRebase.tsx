import { useEffect, useMemo, useState } from "react";
import { api, type BranchInfo, type RebaseAction, type RebaseCommit, type RebaseStep } from "../api";
import { useI18n } from "../i18n";
import { errorMessage, shortOid, type RunOperation } from "../types";

const ACTIONS: RebaseAction[] = ["pick", "reword", "squash", "fixup", "drop"];

export function InteractiveRebase({ repositoryId, initialOnto, onClose, onRun }: { repositoryId: number; initialOnto?: string; onClose: () => void; onRun: RunOperation }) {
  const { t } = useI18n();
  const [branches, setBranches] = useState<BranchInfo[]>([]);
  const [onto, setOnto] = useState("");
  const [commits, setCommits] = useState<RebaseCommit[]>([]);
  const [steps, setSteps] = useState<RebaseStep[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    api.branches(repositoryId).then((all) => {
      const local = all.filter((branch) => !branch.remote);
      setBranches(local);
      const current = local.find((branch) => branch.current);
      setOnto(initialOnto ?? current?.upstream ?? current?.name ?? "");
    }).catch((cause) => setError(errorMessage(cause)));
  }, [repositoryId]);

  useEffect(() => {
    if (!onto) { setCommits([]); setSteps([]); return; }
    setLoading(true); setError("");
    api.rebaseCommits(repositoryId, onto).then((list) => {
      setCommits(list);
      setSteps(list.map((commit) => ({ oid: commit.oid, action: "pick" as RebaseAction })));
    }).catch((cause) => { setCommits([]); setSteps([]); setError(errorMessage(cause)); })
      .finally(() => setLoading(false));
  }, [repositoryId, onto]);

  const subjectByOid = useMemo(() => new Map(commits.map((commit) => [commit.oid, commit.subject])), [commits]);

  const move = (index: number, delta: -1 | 1) => setSteps((current) => {
    const target = index + delta;
    if (target < 0 || target >= current.length) return current;
    const next = [...current];
    [next[index], next[target]] = [next[target], next[index]];
    return next;
  });

  const setAction = (index: number, action: RebaseAction) => setSteps((current) => current.map((step, i) => i === index ? { ...step, action } : step));
  const setMessage = (index: number, message: string) => setSteps((current) => current.map((step, i) => i === index ? { ...step, message } : step));

  const startable = onto.trim() !== "" && steps.some((step) => step.action !== "drop");

  const start = () => {
    const plan = steps.map((step) => step.action === "reword" && !step.message?.trim() ? { ...step, message: subjectByOid.get(step.oid) ?? "" } : step);
    onRun({ type: "interactiveRebase", onto: onto.trim(), plan });
    onClose();
  };

  return <div className="modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}><section className="form-dialog rebase-dialog" role="dialog" aria-modal="true" aria-labelledby="rebase-title" onKeyDown={(event) => { if (event.key === "Escape") onClose(); }}>
    <header><h2 id="rebase-title">{t("interactiveRebase")}</h2></header>
    <label className="dialog-field"><span>{t("onto")}</span><select value={onto} onChange={(event) => setOnto(event.target.value)}>{!branches.some((branch) => branch.name === onto) && onto && <option value={onto}>{onto}</option>}{branches.map((branch) => <option key={branch.name} value={branch.name}>{branch.name}{branch.current ? " *" : ""}</option>)}</select></label>
    {loading && <p>{t("running")}…</p>}
    {error && <p className="dialog-error">{error}</p>}
    {!loading && steps.length > 0 && <div className="rebase-list">{steps.map((step, index) => <div className="rebase-row" key={step.oid}>
      <span className="rebase-order"><button type="button" onClick={() => move(index, -1)} disabled={index === 0}>↑</button><button type="button" onClick={() => move(index, 1)} disabled={index === steps.length - 1}>↓</button></span>
      <select value={step.action} onChange={(event) => setAction(index, event.target.value as RebaseAction)}>{ACTIONS.map((action) => <option key={action} value={action}>{t(action)}</option>)}</select>
      <span className="rebase-commit"><code>{shortOid(step.oid)}</code><span>{subjectByOid.get(step.oid)}</span></span>
      {step.action === "reword" && <input value={step.message ?? ""} placeholder={subjectByOid.get(step.oid)} onChange={(event) => setMessage(index, event.target.value)} />}
    </div>)}</div>}
    {!loading && !error && steps.length === 0 && onto && <p>{t("noCommitsToRebase")}</p>}
    <footer><button type="button" onClick={onClose}>{t("cancel")}</button><button className="danger" onClick={start} disabled={!startable}>{t("startRebase")}</button></footer>
  </section></div>;
}
