import { useMemo } from "react";
import { Decoration, Diff, Hunk, parseDiff, tokenize, type HunkData } from "react-diff-view";
import "react-diff-view/style/index.css";
import refractor from "refractor/core";
import bash from "refractor/lang/bash";
import ini from "refractor/lang/ini";
import json from "refractor/lang/json";
import jsx from "refractor/lang/jsx";
import markdown from "refractor/lang/markdown";
import python from "refractor/lang/python";
import rust from "refractor/lang/rust";
import tsx from "refractor/lang/tsx";
import typescript from "refractor/lang/typescript";
import yaml from "refractor/lang/yaml";
import type { DiffFile, OperationRequest } from "./api";
import { useI18n } from "./i18n";

export type DiffMode = "unified" | "split";

[bash, ini, json, jsx, markdown, python, rust, typescript, tsx, yaml].forEach(refractor.register);

const languageForPath = (path: string) => {
  const extension = path.toLowerCase().split(".").pop() ?? "";
  return ({ ts: "typescript", mts: "typescript", cts: "typescript", tsx: "tsx", js: "javascript", jsx: "jsx", mjs: "javascript", cjs: "javascript", json: "json", html: "markup", htm: "markup", xml: "markup", svg: "markup", css: "css", rs: "rust", py: "python", sh: "bash", bash: "bash", zsh: "bash", md: "markdown", markdown: "markdown", yaml: "yaml", yml: "yaml", toml: "ini" } as Record<string, string>)[extension];
};

export function DiffView({ diff, snapshotId, mode, onModeChange, onBack, onRun, fileActions, onFileHistory, onBlame }: { diff: DiffFile; snapshotId?: number; mode: DiffMode; onModeChange: (mode: DiffMode) => void; onBack: () => void; onRun: (request: OperationRequest) => void | Promise<void>; fileActions?: boolean; onFileHistory?: (path: string) => void; onBlame?: (path: string) => void }) {
  const { t } = useI18n();
  const file = useMemo(() => parseDiff(diff.patch, { nearbySequences: "zip" })[0], [diff.patch]);
  const language = languageForPath(diff.path);
  const tokens = useMemo(() => file && language ? tokenize(file.hunks, { highlight: true, refractor, language }) : null, [file, language]);
  const preamble = diff.patch.slice(0, diff.patch.indexOf("@@")).trimEnd();
  const hunkAction = (hunk: HunkData) => {
    const owned = diff.hunks.find((item) => item.header === hunk.content);
    return owned && snapshotId != null ? <button onClick={() => onRun({ type: diff.staged ? "unstageHunk" : "stageHunk", snapshotId, hunkId: owned.id })}>{diff.staged ? t("unstageHunk") : t("stageHunk")}</button> : null;
  };
  const header = <header className="canvas-header"><button onClick={onBack}>← {t("back")}</button><strong>{diff.path}</strong>{!diff.binary && !diff.tooLarge && <div className="diff-mode" aria-label={t("diffLayout")}><button className={mode === "unified" ? "active" : ""} onClick={() => onModeChange("unified")}>{t("unified")}</button><button className={mode === "split" ? "active" : ""} onClick={() => onModeChange("split")}>{t("sideBySide")}</button></div>}{fileActions && <div className="file-actions"><button onClick={() => onFileHistory?.(diff.path)}>{t("fileHistory")}</button><button onClick={() => onBlame?.(diff.path)}>{t("blame")}</button></div>}<span>{diff.staged ? "INDEX ↔ HEAD" : "WORKTREE ↔ INDEX"}</span></header>;
  if (diff.binary || diff.tooLarge) return <div className="diff-view">{header}<div className="canvas-empty"><h2>{diff.binary ? t("binaryDiff") : t("diffTooLarge")}</h2><button onClick={() => onRun({ type: "runDifftool", path: diff.path, staged: diff.staged })}>{t("openDifftool")}</button></div></div>;
  if (!file?.hunks.length) return <div className="diff-view">{header}<div className="diff-lines raw-diff">{diff.patch.split("\n").map((line, index) => <div className="meta" key={index}><span>{index + 1}</span><code>{line || " "}</code></div>)}</div></div>;
  return <div className="diff-view">{header}<div className="diff-lines"><Diff diffType={file.type} hunks={file.hunks} viewType={mode} tokens={tokens} className="gitdock-diff">{(hunks) => hunks.flatMap((hunk, index) => [index === 0 && <Decoration key="file-header"><pre>{preamble}</pre></Decoration>, <Decoration key={`header-${hunk.content}`}><div className="hunk-decoration"><code>{hunk.content}</code>{hunkAction(hunk)}</div></Decoration>, <Hunk key={hunk.content} hunk={hunk} />]).filter(Boolean) as React.ReactElement[]}</Diff></div></div>;
}
