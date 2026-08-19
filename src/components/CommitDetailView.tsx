import { useMemo } from "react";
import type { CommitDetail, CommitFileChange } from "../api";
import { useI18n } from "../i18n";

function fileName(path: string) {
  return path.split("/").at(-1) || path;
}

function stats(file: CommitFileChange, binaryLabel: string) {
  if (file.additions == null && file.deletions == null) return <span className="commit-file-binary">{binaryLabel}</span>;
  return <span className="commit-file-stats"><span className="commit-stat-add">+{file.additions ?? 0}</span><span className="commit-stat-del">−{file.deletions ?? 0}</span></span>;
}

export function CommitDetailView({ detail, onBack, onOpenFile }: { detail: CommitDetail; onBack: () => void; onOpenFile: (path: string) => void }) {
  const { language, t } = useI18n();
  const dateFormatter = useMemo(() => new Intl.DateTimeFormat(language, { dateStyle: "medium", timeStyle: "short" }), [language]);
  return <div className="commit-detail">
    <header className="canvas-header"><button onClick={onBack}>← {t("back")}</button></header>
    <section className="commit-detail-info">
      <div className="pane-title"><span>{t("commitDiff")}</span></div>
      <dl className="commit-detail-meta">
        <div><dt>{t("commitId")}</dt><dd><code>{detail.oid}</code></dd></div>
        <div><dt>{t("author")}</dt><dd>{detail.author}{detail.email ? ` <${detail.email}>` : ""}</dd></div>
        <div><dt>{t("date")}</dt><dd><time dateTime={detail.authoredAt}>{dateFormatter.format(new Date(detail.authoredAt))}</time></dd></div>
        <div><dt>{t("commitMessage")}</dt><dd><pre>{detail.message}</pre></dd></div>
      </dl>
    </section>
    <section className="commit-detail-files">
      <div className="pane-title"><span>{t("changedFiles")}</span><code>{detail.files.length}</code></div>
      <div className="commit-file-list">{detail.files.map((file) => <div className="file-row commit-file-row" key={file.path}><button className="file-main" onClick={() => onOpenFile(file.path)}><b>{fileName(file.path)}</b><small>{file.originalPath ? `${file.originalPath} → ${file.path}` : file.path}</small></button>{stats(file, t("binaryFile"))}</div>)}</div>
    </section>
  </div>;
}
