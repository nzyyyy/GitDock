import { useEffect, useMemo, useState } from "react";
import type { HLJSApi, LanguageFn } from "highlight.js";
import type { DiffFile, OperationRequest } from "./api";
import { useI18n } from "./i18n";

export type DiffMode = "unified" | "split";

type LineKind = "add" | "delete" | "context" | "hunk" | "file-header" | "meta";
type ParsedLine = { kind: LineKind; text: string; path: string; oldLine?: number; newLine?: number; hunk?: string };
type SplitLine = { kind: LineKind; left?: ParsedLine; right?: ParsedLine; full?: ParsedLine };

const languageLoaders: Record<string, () => Promise<LanguageFn>> = {
  typescript: () => import("highlight.js/lib/languages/typescript").then((module) => module.default),
  javascript: () => import("highlight.js/lib/languages/javascript").then((module) => module.default),
  json: () => import("highlight.js/lib/languages/json").then((module) => module.default),
  xml: () => import("highlight.js/lib/languages/xml").then((module) => module.default),
  css: () => import("highlight.js/lib/languages/css").then((module) => module.default),
  rust: () => import("highlight.js/lib/languages/rust").then((module) => module.default),
  python: () => import("highlight.js/lib/languages/python").then((module) => module.default),
  bash: () => import("highlight.js/lib/languages/bash").then((module) => module.default),
  markdown: () => import("highlight.js/lib/languages/markdown").then((module) => module.default),
  yaml: () => import("highlight.js/lib/languages/yaml").then((module) => module.default),
  ini: () => import("highlight.js/lib/languages/ini").then((module) => module.default),
};

let highlighter: HLJSApi | undefined;
const languagePromises = new Map<string, Promise<void>>();

const languageForPath = (path: string) => {
  const extension = path.toLowerCase().split(".").pop();
  if (["ts", "tsx", "mts", "cts"].includes(extension ?? "")) return "typescript";
  if (["js", "jsx", "mjs", "cjs"].includes(extension ?? "")) return "javascript";
  if (extension === "json") return "json";
  if (["html", "htm", "xml", "svg"].includes(extension ?? "")) return "xml";
  if (extension === "css") return "css";
  if (extension === "rs") return "rust";
  if (extension === "py") return "python";
  if (["sh", "bash", "zsh"].includes(extension ?? "")) return "bash";
  if (["md", "markdown"].includes(extension ?? "")) return "markdown";
  if (["yaml", "yml"].includes(extension ?? "")) return "yaml";
  if (extension === "toml") return "ini";
};

const loadLanguage = (language: string) => {
  const existing = languagePromises.get(language);
  if (existing) return existing;
  const promise = Promise.all([import("highlight.js/lib/core"), languageLoaders[language]()]).then(([core, grammar]) => {
    highlighter = core.default;
    if (!highlighter.getLanguage(language)) highlighter.registerLanguage(language, grammar);
  });
  languagePromises.set(language, promise);
  return promise;
};

export function parseUnifiedPatch(patch: string, defaultPath: string) {
  let path = defaultPath;
  let oldLine: number | undefined;
  let newLine: number | undefined;
  let valid = false;
  const lines = (patch.endsWith("\n") ? patch.slice(0, -1) : patch).split("\n").map<ParsedLine>((text) => {
    if (text.startsWith("diff --git ")) {
      oldLine = undefined; newLine = undefined;
      return { kind: "file-header", text, path };
    }
    if (text.startsWith("+++ ")) {
      const next = text.slice(4).replace(/^b\//, "");
      if (next !== "/dev/null") path = next;
      return { kind: "file-header", text, path };
    }
    if (text.startsWith("--- ")) return { kind: "file-header", text, path };
    const hunk = text.match(/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
    if (hunk) {
      valid = true;
      oldLine = Number(hunk[1]); newLine = Number(hunk[2]);
      return { kind: "hunk", text, path, hunk: text };
    }
    if (oldLine !== undefined && newLine !== undefined) {
      if (text.startsWith("-")) return { kind: "delete", text: text.slice(1), path, oldLine: oldLine++ };
      if (text.startsWith("+")) return { kind: "add", text: text.slice(1), path, newLine: newLine++ };
      if (text.startsWith(" ")) return { kind: "context", text: text.slice(1), path, oldLine: oldLine++, newLine: newLine++ };
    }
    return { kind: "meta", text, path };
  });
  return { valid, lines };
}

export function alignSplitLines(lines: ParsedLine[]) {
  const result: SplitLine[] = [];
  for (let index = 0; index < lines.length;) {
    const line = lines[index];
    if (line.kind === "delete") {
      const deleted: ParsedLine[] = [];
      const added: ParsedLine[] = [];
      while (lines[index]?.kind === "delete") deleted.push(lines[index++]);
      while (lines[index]?.kind === "add") added.push(lines[index++]);
      for (let pair = 0; pair < Math.max(deleted.length, added.length); pair += 1) {
        result.push({ kind: deleted[pair] && added[pair] ? "context" : deleted[pair] ? "delete" : "add", left: deleted[pair], right: added[pair] });
      }
    } else if (line.kind === "add") {
      result.push({ kind: "add", right: line }); index += 1;
    } else if (line.kind === "context") {
      result.push({ kind: "context", left: line, right: line }); index += 1;
    } else {
      result.push({ kind: line.kind, full: line }); index += 1;
    }
  }
  return result;
}

function CodeLine({ line }: { line?: ParsedLine }) {
  if (!line) return <code aria-hidden="true"> </code>;
  const language = ["add", "delete", "context"].includes(line.kind) ? languageForPath(line.path) : undefined;
  const html = language && highlighter?.getLanguage(language)
    ? highlighter.highlight(line.text || " ", { language, ignoreIllegals: true }).value
    : undefined;
  return html ? <code dangerouslySetInnerHTML={{ __html: html }} /> : <code>{line.text || " "}</code>;
}

export function DiffView({ diff, snapshotId, mode, onModeChange, onBack, onRun, fileActions, onFileHistory, onBlame }: { diff: DiffFile; snapshotId?: number; mode: DiffMode; onModeChange: (mode: DiffMode) => void; onBack: () => void; onRun: (request: OperationRequest) => void | Promise<void>; fileActions?: boolean; onFileHistory?: (path: string) => void; onBlame?: (path: string) => void }) {
  const { t } = useI18n();
  const parsed = useMemo(() => parseUnifiedPatch(diff.patch, diff.path), [diff.patch, diff.path]);
  const split = useMemo(() => alignSplitLines(parsed.lines), [parsed.lines]);
  const [, setHighlightRevision] = useState(0);
  useEffect(() => {
    const languages = [...new Set(parsed.lines.flatMap((line) => {
      const language = languageForPath(line.path);
      return language ? [language] : [];
    }))];
    let current = true;
    Promise.allSettled(languages.map(loadLanguage)).then(() => { if (current) setHighlightRevision((revision) => revision + 1); });
    return () => { current = false; };
  }, [parsed.lines]);
  const hunkAction = (header?: string) => {
    if (!header || !snapshotId) return null;
    const hunk = diff.hunks.find((item) => item.header === header);
    return hunk && <button onClick={() => onRun({ type: diff.staged ? "unstageHunk" : "stageHunk", snapshotId, hunkId: hunk.id })}>{diff.staged ? t("unstageHunk") : t("stageHunk")}</button>;
  };
  const header = <header className="canvas-header"><button onClick={onBack}>← {t("back")}</button><strong>{diff.path}</strong>{!diff.binary && !diff.tooLarge && <div className="diff-mode" aria-label={t("diffLayout")}><button className={mode === "unified" ? "active" : ""} onClick={() => onModeChange("unified")}>{t("unified")}</button><button className={mode === "split" ? "active" : ""} onClick={() => onModeChange("split")}>{t("sideBySide")}</button></div>}{fileActions && <div className="file-actions"><button onClick={() => onFileHistory?.(diff.path)}>{t("fileHistory")}</button><button onClick={() => onBlame?.(diff.path)}>{t("blame")}</button></div>}<span>{diff.staged ? "INDEX ↔ HEAD" : "WORKTREE ↔ INDEX"}</span></header>;
  if (diff.binary || diff.tooLarge) return <div className="diff-view">{header}<div className="canvas-empty"><h2>{diff.binary ? t("binaryDiff") : t("diffTooLarge")}</h2><button onClick={() => onRun({ type: "runDifftool", path: diff.path, staged: diff.staged })}>{t("openDifftool")}</button></div></div>;
  if (!parsed.valid) return <div className="diff-view">{header}<div className="diff-lines raw-diff">{diff.patch.split("\n").map((line, index) => <div className="meta" key={index}><span>{index + 1}</span><code>{line || " "}</code></div>)}</div></div>;
  return <div className="diff-view">{header}{mode === "unified" ? <div className="diff-lines unified-diff">{parsed.lines.map((line, index) => <div className={line.kind} key={index}><span>{line.oldLine ?? ""}</span><span>{line.newLine ?? ""}</span><CodeLine line={line} />{line.kind === "hunk" && hunkAction(line.hunk)}</div>)}</div> : <div className="diff-lines split-diff">{split.map((row, index) => row.full ? <div className={`${row.kind} split-full`} key={index}><code>{row.full.text || " "}</code>{row.kind === "hunk" && hunkAction(row.full.hunk)}</div> : <div className={row.kind} key={index}><span>{row.left?.oldLine ?? ""}</span><CodeLine line={row.left} /><span>{row.right?.newLine ?? ""}</span><CodeLine line={row.right} /></div>)}</div>}</div>;
}
