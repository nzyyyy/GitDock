import { memo, useEffect, useState } from "react";
import { api, type RepositorySummary, type StashInfo } from "../api";
import { useI18n } from "../i18n";
import { errorMessage, type DialogSpec, type RunOperation } from "../types";

export function StashCanvas({ repository }: { repository?: RepositorySummary }) { const { t } = useI18n(); return <div className="canvas-empty"><div className="change-tally"><strong>≋</strong><span>{t("savedStates")}</span></div><h2>{repository?.name} {t("stashes")}</h2><p>{t("stashHint")}</p></div>; }

export function StashesPane({ repositoryId, onRun, onDialog, onError }: { repositoryId: number; onRun: RunOperation; onDialog: (spec: DialogSpec) => void; onError: (message: string) => void }) {
  const { t } = useI18n();
  const [stashes, setStashes] = useState<StashInfo[]>([]);
  useEffect(() => { api.getStashes(repositoryId).then(setStashes).catch((error) => onError(errorMessage(error))); }, [repositoryId, onError]);
  const create = () => onDialog({ title: t("create"), fields: [{ name: "message", label: t("stashMessage") }, { name: "includeUntracked", label: t("includeUntracked"), value: false, type: "checkbox" }], onSubmit: ({ message, includeUntracked }) => onRun({ type: "stashCreate", message: String(message).trim() || null, includeUntracked: Boolean(includeUntracked) }) });
  return <div><div className="pane-title"><span>{t("stashes")}</span><button onClick={create}>＋</button></div><div className="object-list">{stashes.map((stash) => <div className="stash-row" key={stash.oid}><button><strong>stash@{`{${stash.index}}`}</strong><span>{stash.subject}</span></button><div><button onClick={() => onRun({ type: "stashApply", index: stash.index, pop: false })}>{t("apply")}</button><button onClick={() => onRun({ type: "stashApply", index: stash.index, pop: true })}>{t("pop")}</button><button onClick={() => onRun({ type: "stashDrop", index: stash.index })}>{t("drop")}</button></div></div>)}</div></div>;
}

export const MemoStashesPane = memo(StashesPane);
