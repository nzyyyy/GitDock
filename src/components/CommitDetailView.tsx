import { useMemo } from "react";
import type { CommitDetail } from "../api";
import { useI18n } from "../i18n";
import { ChangedFileList } from "./ChangedFileList";

export function CommitDetailView({ detail, kind = "commit", onBack, onOpenFile }: { detail: CommitDetail; kind?: "commit" | "stash"; onBack: () => void; onOpenFile: (path: string) => void }) {
  const { language, t } = useI18n();
  const dateFormatter = useMemo(() => new Intl.DateTimeFormat(language, { dateStyle: "medium", timeStyle: "short" }), [language]);
  return <div className="commit-detail">
    <header className="canvas-header"><button onClick={onBack}>← {t("back")}</button></header>
    <section className="commit-detail-info">
      <div className="pane-title"><span>{t(kind === "stash" ? "stashDiff" : "commitDiff")}</span></div>
      <dl className="commit-detail-meta">
        <div><dt>{t(kind === "stash" ? "stashId" : "commitId")}</dt><dd><code>{detail.oid}</code></dd></div>
        <div><dt>{t("author")}</dt><dd>{detail.author}{detail.email ? ` <${detail.email}>` : ""}</dd></div>
        <div><dt>{t("date")}</dt><dd><time dateTime={detail.authoredAt}>{dateFormatter.format(new Date(detail.authoredAt))}</time></dd></div>
        <div><dt>{t(kind === "stash" ? "stashMessage" : "commitMessage")}</dt><dd><pre>{detail.message}</pre></dd></div>
      </dl>
    </section>
    <ChangedFileList files={detail.files} onOpenFile={onOpenFile} />
  </div>;
}
