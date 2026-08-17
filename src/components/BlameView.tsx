import { useMemo } from "react";
import type { BlameFile, BlameHunk } from "../api";
import { useI18n } from "../i18n";
import { shortOid } from "../types";

export function BlameView({ blame, onBack }: { blame: BlameFile; onBack: () => void }) {
  const { language, t } = useI18n();
  const dateFormatter = useMemo(() => new Intl.DateTimeFormat(language), [language]);
  const lineHunks = useMemo(() => {
    const result: Array<BlameHunk | undefined> = new Array(blame.content.length);
    for (const hunk of blame.hunks) {
      for (let line = hunk.startLine; line < hunk.startLine + hunk.lineCount; line += 1) result[line - 1] = hunk;
    }
    return result;
  }, [blame]);
  return <div className="blame-view">
    <header className="canvas-header"><button onClick={onBack}>← {t("back")}</button><strong>{t("blameOf")} {blame.path}</strong></header>
    <div className="blame-list">{blame.content.map((line, index) => {
      const hunk = lineHunks[index];
      const date = hunk ? dateFormatter.format(new Date(hunk.authorTime * 1000)) : "";
      return <div className="blame-row" key={index}><span className="blame-author">{hunk && hunk.startLine === index + 1 ? `${hunk.author} · ${shortOid(hunk.oid)} · ${date}` : ""}</span><span className="blame-line-number">{index + 1}</span><code>{line || " "}</code></div>;
    })}</div>
  </div>;
}
