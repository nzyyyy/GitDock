import { useMemo } from "react";
import { Decoration, Diff, Hunk, getChangeKey, parseDiff, tokenize, type ChangeData } from "react-diff-view";
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
import type { DiffFile, DiffHunk, OperationRequest } from "./api";
import { useI18n } from "./i18n";
import type { OperationFinished } from "./types";

[bash, ini, json, jsx, markdown, python, rust, typescript, tsx, yaml].forEach(refractor.register);

const languageForPath = (path: string) => {
  const extension = path.toLowerCase().split(".").pop() ?? "";
  return ({ ts: "typescript", mts: "typescript", cts: "typescript", tsx: "tsx", js: "javascript", jsx: "jsx", mjs: "javascript", cjs: "javascript", json: "json", html: "markup", htm: "markup", xml: "markup", svg: "markup", css: "css", rs: "rust", py: "python", sh: "bash", bash: "bash", zsh: "bash", md: "markdown", markdown: "markdown", yaml: "yaml", yml: "yaml", toml: "ini" } as Record<string, string>)[extension];
};

const AT = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;

function islandKeyFromPatch(patch: string): string | undefined {
  const lines = patch.split("\n");
  const headerAt = lines.findIndex((line) => line.startsWith("@@"));
  if (headerAt < 0) return undefined;
  const match = AT.exec(lines[headerAt]);
  if (!match) return undefined;
  let old = Number(match[1]);
  let next = Number(match[2]);
  let del = 0;
  let ins = 0;
  for (const line of lines.slice(headerAt + 1)) {
    if (line.startsWith("\\")) continue;
    if (line.startsWith("-")) {
      if (!del) del = old;
      old += 1;
    } else if (line.startsWith("+")) {
      if (!ins) ins = next;
      next += 1;
    } else {
      old += 1;
      next += 1;
    }
  }
  return `${del}:${ins}`;
}

function islandKeyFromChanges(changes: ChangeData[]): string {
  const del = changes.find((change) => change.type === "delete");
  const ins = changes.find((change) => change.type === "insert");
  return `${del?.type === "delete" ? del.lineNumber : 0}:${ins?.type === "insert" ? ins.lineNumber : 0}`;
}

function changeIslands(changes: ChangeData[]): ChangeData[][] {
  const islands: ChangeData[][] = [];
  let current: ChangeData[] = [];
  for (const change of changes) {
    if (change.type === "normal") {
      if (current.length) islands.push(current);
      current = [];
    } else current.push(change);
  }
  if (current.length) islands.push(current);
  return islands;
}

function newLineNumber(change: ChangeData): number | undefined {
  if (change.type === "insert") return change.lineNumber;
  if (change.type === "normal") return change.newLineNumber;
  return undefined;
}

function firstChangeHunks(fileHunks: { changes: ChangeData[] }[], owned: DiffHunk[]): Map<string, DiffHunk> {
  const byKey = new Map<string, DiffHunk>();
  for (const hunk of owned) {
    const key = islandKeyFromPatch(hunk.patch);
    if (key) byKey.set(key, hunk);
  }
  const first = new Map<string, DiffHunk>();
  for (const hunk of fileHunks) {
    for (const island of changeIslands(hunk.changes)) {
      const match = byKey.get(islandKeyFromChanges(island));
      if (match && island[0]) first.set(getChangeKey(island[0]), match);
    }
  }
  return first;
}

export function DiffView({ diff, snapshotId, onBack, onRun, onHunkSettled, fileActions, onFileHistory, onBlame, caption }: { diff: DiffFile; snapshotId?: number; onBack: () => void; onRun: (request: OperationRequest, onFinished?: OperationFinished) => void | Promise<void>; onHunkSettled?: () => void; fileActions?: boolean; onFileHistory?: (path: string) => void; onBlame?: (path: string) => void; caption?: string }) {
  const { t } = useI18n();
  const file = useMemo(() => {
    const start = diff.patch.indexOf("diff --git ");
    return start < 0 ? undefined : parseDiff(diff.patch.slice(start))[0];
  }, [diff.patch]);
  const language = languageForPath(diff.path);
  const tokens = useMemo(() => file && language ? tokenize(file.hunks, { highlight: true, refractor, language }) : null, [file, language]);
  const preamble = diff.patch.slice(0, diff.patch.indexOf("@@")).trimEnd();
  const islandStarts = useMemo(() => file ? firstChangeHunks(file.hunks, diff.hunks) : new Map<string, DiffHunk>(), [file, diff.hunks]);
  const runHunk = (hunkId: string) => {
    if (snapshotId == null) return;
    onRun({ type: diff.staged ? "unstageHunk" : "stageHunk", snapshotId, hunkId }, (outcome) => { if (outcome === "succeeded") onHunkSettled?.(); });
  };
  const header = <header className="canvas-header"><button onClick={onBack}>← {t("back")}</button><strong>{diff.path}</strong>{fileActions && <div className="file-actions"><button onClick={() => onFileHistory?.(diff.path)}>{t("fileHistory")}</button><button onClick={() => onBlame?.(diff.path)}>{t("blame")}</button></div>}<span>{caption ?? (diff.staged ? "INDEX ↔ HEAD" : "WORKTREE ↔ INDEX")}</span></header>;
  if (diff.binary || diff.tooLarge) return <div className="diff-view">{header}<div className="canvas-empty"><h2>{diff.binary ? t("binaryDiff") : t("diffTooLarge")}</h2><button onClick={() => onRun({ type: "runDifftool", path: diff.path, staged: diff.staged })}>{t("openDifftool")}</button></div></div>;
  if (!file?.hunks.length) return <div className="diff-view">{header}<div className="diff-lines raw-diff">{diff.patch.split("\n").map((line, index) => <div className="meta" key={index}><span>{index + 1}</span><code>{line || " "}</code></div>)}</div></div>;
  return <div className="diff-view">{header}<div className="diff-lines"><Diff diffType={file.type} hunks={file.hunks} viewType="unified" tokens={tokens} className="gitdock-diff" renderGutter={({ change, side }) => {
    if (side === "old") {
      const owned = islandStarts.get(getChangeKey(change));
      if (!owned || snapshotId == null) return null;
      return <button type="button" className={diff.staged ? "diff-stage unstage" : "diff-stage"} aria-label={diff.staged ? t("unstageHunk") : t("stageHunk")} onClick={() => runHunk(owned.id)}>{diff.staged ? "−" : "+"}</button>;
    }
    const line = newLineNumber(change);
    return line == null ? null : String(line);
  }}>{(hunks) => hunks.flatMap((hunk, index) => [index === 0 && <Decoration key="file-header"><pre>{preamble}</pre></Decoration>, <Decoration key={`header-${hunk.content}`}><div className="hunk-decoration"><code>{hunk.content}</code></div></Decoration>, <Hunk key={hunk.content} hunk={hunk} />]).filter(Boolean) as React.ReactElement[]}</Diff></div></div>;
}
