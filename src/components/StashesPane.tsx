import { memo, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, type RepositorySummary, type StashInfo } from "../api";
import { useI18n } from "../i18n";
import { errorMessage, type DialogSpec, type RunOperation } from "../types";

export function StashCanvas({ repository }: { repository?: RepositorySummary }) { const { t } = useI18n(); return <div className="canvas-empty"><div className="change-tally"><strong>≋</strong><span>{t("savedStates")}</span></div><h2>{repository?.name} {t("stashes")}</h2><p>{t("stashHint")}</p></div>; }

export function StashesPane({ repositoryId, onRun, onDialog, onError }: { repositoryId: number; onRun: RunOperation; onDialog: (spec: DialogSpec) => void; onError: (message: string) => void }) {
  const { t } = useI18n();
  const [stashes, setStashes] = useState<StashInfo[]>();
  useEffect(() => {
    let cancelled = false;
    let generation = 0;
    setStashes(undefined);
    const load = () => {
      const current = ++generation;
      api.getStashes(repositoryId).then((next) => { if (!cancelled && current === generation) setStashes(next); }).catch((error) => { if (!cancelled && current === generation) { setStashes([]); onError(errorMessage(error)); } });
    };
    load();
    const unlisten = listen<{ repositoryId: number }>("repository-changed", ({ payload }) => { if (payload.repositoryId === repositoryId) load(); });
    return () => { cancelled = true; void unlisten.then((stop) => stop()); };
  }, [repositoryId, onError]);
  const create = () => onDialog({ title: t("create"), fields: [{ name: "message", label: t("stashMessage") }, { name: "includeUntracked", label: t("includeUntracked"), value: false, type: "checkbox" }], onSubmit: ({ message, includeUntracked }) => onRun({ type: "stashCreate", message: String(message).trim() || null, includeUntracked: Boolean(includeUntracked) }) });
  return <div><div className="pane-title"><span>{t("stashes")}</span><button aria-label={`${t("create")} ${t("stashes")}`} onClick={create}>＋</button></div><div className="object-list">{stashes === undefined ? <div className="skeleton-list" role="status" aria-label={t("loading")}>{[0, 1, 2, 3].map((index) => <div className="skeleton-row object" key={index} />)}</div> : stashes.length === 0 ? <p className="pane-empty">{t("noStashes")}</p> : stashes.map((stash) => <div className="stash-row" key={stash.oid}><div className="object-copy"><strong>stash@{`{${stash.index}}`}</strong><span>{stash.subject}</span></div><div><button onClick={() => onRun({ type: "stashApply", index: stash.index, pop: false })}>{t("apply")}</button><button onClick={() => onRun({ type: "stashApply", index: stash.index, pop: true })}>{t("pop")}</button><button onClick={() => onRun({ type: "stashDrop", index: stash.index })}>{t("drop")}</button></div></div>)}</div></div>;
}

export const MemoStashesPane = memo(StashesPane);
