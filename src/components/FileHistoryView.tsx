import { useMemo } from "react";
import { useI18n } from "../i18n";
import { shortOid } from "../types";
import type { FileHistoryEntry } from "../api";

export function FileHistoryView({ path, entries, selectedOid, diff, onBack, onSelect }: {
  path: string;
  entries: FileHistoryEntry[];
  selectedOid?: string;
  diff?: string;
  onBack: () => void;
  onSelect: (oid: string) => void;
}) {
  const { language, t } = useI18n();
  const dateFormatter = useMemo(() => new Intl.DateTimeFormat(language), [language]);
  return <div className="file-history-view">
    <header className="canvas-header"><button onClick={onBack}>← {t("back")}</button><strong>{t("historyOf")} {path}</strong><span>{entries.length}</span></header>
    <div className="object-list">{entries.map((entry) => <div className={`object-action-row ${selectedOid === entry.oid ? "selected" : ""}`} key={entry.oid}><button onClick={() => onSelect(entry.oid)}><strong>{entry.subject}</strong><span>{entry.author} · {shortOid(entry.oid)} · {dateFormatter.format(new Date(entry.authoredAt))}</span></button></div>)}{entries.length === 0 && <p className="canvas-empty">{t("noHistory")}</p>}</div>
    {diff !== undefined && <pre className="diff-lines raw-diff">{diff || " "}</pre>}
  </div>;
}
