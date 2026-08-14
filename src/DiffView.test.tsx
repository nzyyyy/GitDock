import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, expect, test, vi } from "vitest";
import type { DiffFile, OperationRequest } from "./api";
import { DiffView, type DiffMode } from "./DiffView";
import { I18nProvider } from "./i18n";

const patch = "diff --git a/src/file.ts b/src/file.ts\n--- a/src/file.ts\n+++ b/src/file.ts\n@@ -2,3 +2,2 @@\n-const oldValue = 1;\n-const removed = true;\n+const newValue = 2;\n context\n\\ No newline at end of file";
const diff: DiffFile = { path: "src/file.ts", staged: false, binary: false, tooLarge: false, patch, hunks: [{ id: "h1", header: "@@ -2,3 +2,2 @@", patch }] };
afterEach(cleanup);

function View({ value = diff, onRun = vi.fn(), initialMode = "unified", snapshotId = 7 }: { value?: DiffFile; onRun?: (request: OperationRequest) => void; initialMode?: DiffMode; snapshotId?: number }) {
  const [mode, setMode] = useState(initialMode);
  return <I18nProvider language="en"><DiffView diff={value} snapshotId={snapshotId} mode={mode} onModeChange={setMode} onBack={() => {}} onRun={onRun} /></I18nProvider>;
}

test("renders file headers, no-newline patches, and asymmetric changes in both layouts", () => {
  render(<View />);
  expect(screen.getByText(/diff --git a\/src\/file\.ts/)).toBeInTheDocument();
  expect([...document.querySelectorAll(".diff-code-delete")].some((cell) => cell.textContent === "const removed = true;")).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Side by side" }));
  expect([...document.querySelectorAll(".diff-code-insert")].some((cell) => cell.textContent === "const newValue = 2;")).toBe(true);
  expect(document.querySelector(".diff-split")).toBeInTheDocument();
});

test("maps staged and unstaged hunk actions to backend-owned ids", () => {
  const onRun = vi.fn();
  const { rerender } = render(<View onRun={onRun} />);
  fireEvent.click(screen.getByRole("button", { name: "Stage hunk" }));
  expect(onRun).toHaveBeenLastCalledWith({ type: "stageHunk", snapshotId: 7, hunkId: "h1" });
  rerender(<View value={{ ...diff, staged: true }} onRun={onRun} />);
  fireEvent.click(screen.getByRole("button", { name: "Unstage hunk" }));
  expect(onRun).toHaveBeenLastCalledWith({ type: "unstageHunk", snapshotId: 7, hunkId: "h1" });
});

test("keeps syntax highlighting on code but not hunk headers", () => {
  render(<View />);
  expect(document.querySelector(".diff-code .token.keyword")).toBeInTheDocument();
  expect(document.querySelector(".hunk-decoration .token")).not.toBeInTheDocument();
});

test("falls back to raw text when a patch has no valid hunk", () => {
  render(<View value={{ ...diff, path: "unknown.bin", patch: "not a unified patch", hunks: [] }} />);
  expect(screen.getByText("not a unified patch")).toBeInTheDocument();
  expect(document.querySelector(".token.keyword")).not.toBeInTheDocument();
});

test.each([["binary", { binary: true, tooLarge: false }, "Binary diff"], ["too large", { binary: false, tooLarge: true }, "Diff exceeds the safe preview limit"]])("keeps the %s fallback", (_name, state, message) => {
  render(<View value={{ ...diff, ...state }} />);
  expect(screen.getByRole("heading", { name: message })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Open configured difftool" })).toBeInTheDocument();
});
