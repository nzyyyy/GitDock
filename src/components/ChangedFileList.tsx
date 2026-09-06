import type { CommitFileChange } from "../api";
import { useI18n } from "../i18n";

function fileName(path: string) {
  return path.split("/").at(-1) || path;
}

function stats(file: CommitFileChange, binaryLabel: string) {
  if (file.additions == null && file.deletions == null) return <span className="commit-file-binary">{binaryLabel}</span>;
  return <span className="commit-file-stats"><span className="commit-stat-add">+{file.additions ?? 0}</span><span className="commit-stat-del">−{file.deletions ?? 0}</span></span>;
}

export function ChangedFileList({ files, onOpenFile }: { files: CommitFileChange[]; onOpenFile: (path: string) => void }) {
  const { t } = useI18n();
  return <section className="commit-detail-files">
    <div className="pane-title"><span>{t("changedFiles")}</span><code>{files.length}</code></div>
    <div className="commit-file-list">{files.map((file) => <div className="file-row commit-file-row" key={file.path}><button className="file-main" onClick={() => onOpenFile(file.path)}><b>{fileName(file.path)}</b><small>{file.originalPath ? `${file.originalPath} → ${file.path}` : file.path}</small></button>{stats(file, t("binaryFile"))}</div>)}</div>
  </section>;
}
