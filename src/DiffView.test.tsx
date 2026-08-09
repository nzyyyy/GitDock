import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, test, vi } from "vitest";
import type { DiffFile, OperationRequest } from "./api";
import { alignSplitLines, DiffView, parseUnifiedPatch, type DiffMode } from "./DiffView";
import { I18nProvider } from "./i18n";

const patch = "diff --git a/src/file.ts b/src/file.ts\n--- a/src/file.ts\n+++ b/src/file.ts\n@@ -2,2 +2,2 @@\n-const oldValue = 1;\n+const newValue = 2;\n context";
const diff: DiffFile = { path: "src/file.ts", staged: false, binary: false, tooLarge: false, patch, hunks: [{ id: "h1", header: "@@ -2,2 +2,2 @@", patch }] };
afterEach(cleanup);

test("parses real line numbers and aligns replacement rows", () => {
  const parsed = parseUnifiedPatch(patch, diff.path);
  expect(parsed.valid).toBe(true);
  expect(parsed.lines.find((line) => line.kind === "delete")).toMatchObject({ oldLine: 2, path: "src/file.ts", text: "const oldValue = 1;" });
  expect(parsed.lines.find((line) => line.kind === "add")).toMatchObject({ newLine: 2, path: "src/file.ts", text: "const newValue = 2;" });
  const replacement = alignSplitLines(parsed.lines).find((line) => line.left?.kind === "delete");
  expect(replacement?.right?.kind).toBe("add");
});

test("parses multiple files and preserves no-newline markers", () => {
  const parsed = parseUnifiedPatch(`${patch}\n\\ No newline at end of file\ndiff --git a/data.json b/data.json\n--- a/data.json\n+++ b/data.json\n@@ -1 +1 @@\n-{\"old\":true}\n+{\"new\":true}`, "src/file.ts");
  expect(parsed.lines.find((line) => line.text === '{"new":true}')).toMatchObject({ path: "data.json", newLine: 1 });
  expect(parsed.lines.some((line) => line.kind === "meta" && line.text.includes("No newline"))).toBe(true);
});

test("does not turn a trailing patch newline into a context row", () => {
  const parsed = parseUnifiedPatch(`${patch}\n`, diff.path);
  expect(parsed.lines.at(-1)).toMatchObject({ kind: "context", text: "context", oldLine: 3, newLine: 3 });
});

describe("DiffView", () => {
  function View({ onRun = vi.fn() }: { onRun?: (request: OperationRequest) => void }) {
    const [mode, setMode] = useState<DiffMode>("unified");
    return <I18nProvider language="en"><DiffView diff={diff} snapshotId={7} mode={mode} onModeChange={setMode} onBack={() => {}} onRun={onRun} /></I18nProvider>;
  }

  test("switches to the dual-rail view", () => {
    render(<View />);
    expect(document.querySelector(".unified-diff")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Side by side" }));
    expect(document.querySelector(".split-diff")).toBeInTheDocument();
    expect(screen.getByText("const oldValue = 1;")).toBeInTheDocument();
    expect(screen.getByText("const newValue = 2;")).toBeInTheDocument();
  });

  test("keeps hunk actions on backend-owned ids", () => {
    const onRun = vi.fn();
    render(<View onRun={onRun} />);
    fireEvent.click(screen.getByRole("button", { name: "Stage hunk" }));
    expect(onRun).toHaveBeenCalledWith({ type: "stageHunk", snapshotId: 7, hunkId: "h1" });
  });

  test("loads known-language highlighting without changing diff prefixes", async () => {
    render(<View />);
    await waitFor(() => expect(document.querySelector(".delete .hljs-keyword")).toBeInTheDocument());
    expect(document.querySelector(".hunk .hljs-number")).not.toBeInTheDocument();
  });

  test("falls back to raw text when a patch cannot be parsed", () => {
    const raw = { ...diff, path: "unknown.bin", patch: "not a unified patch", hunks: [] };
    render(<I18nProvider language="en"><DiffView diff={raw} snapshotId={7} mode="unified" onModeChange={() => {}} onBack={() => {}} onRun={() => {}} /></I18nProvider>);
    expect(screen.getByText("not a unified patch")).toBeInTheDocument();
    expect(document.querySelector(".hljs-keyword")).not.toBeInTheDocument();
  });
});
