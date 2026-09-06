import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import type { DiffFile, OperationRequest } from "./api";
import { BranchComparisonView, DiffView } from "./DiffView";
import { I18nProvider } from "./i18n";

const patch = "diff --git a/src/file.ts b/src/file.ts\n--- a/src/file.ts\n+++ b/src/file.ts\n@@ -2,3 +2,2 @@\n-const oldValue = 1;\n-const removed = true;\n+const newValue = 2;\n context\n\\ No newline at end of file";
const diff: DiffFile = { path: "src/file.ts", staged: false, binary: false, tooLarge: false, patch, hunks: [{ id: "h1", header: "@@ -2,3 +2,2 @@", patch }] };
afterEach(cleanup);

function View({ value = diff, companion, onRun = vi.fn(), onHunkSettled, snapshotId = 7 }: { value?: DiffFile; companion?: DiffFile; onRun?: (request: OperationRequest, onFinished?: (outcome: "succeeded" | "failed" | "cancelled") => void) => void; onHunkSettled?: () => void; snapshotId?: number }) {
  return <I18nProvider language="en"><DiffView diff={value} companionDiff={companion} snapshotId={snapshotId} onBack={() => {}} onRun={onRun} onHunkSettled={onHunkSettled} /></I18nProvider>;
}

test("renders file headers, no-newline patches, and grouped replacements", () => {
  const grouped = "diff --git a/src/file.ts b/src/file.ts\n--- a/src/file.ts\n+++ b/src/file.ts\n@@ -1,2 +1,2 @@\n-oldA\n-oldB\n+newA\n+newB\n";
  render(<View value={{ ...diff, patch: grouped, hunks: [{ id: "h1", header: "@@ -1,2 +1,2 @@", patch: grouped }] }} />);
  expect(screen.getByText(/diff --git a\/src\/file\.ts/)).toBeInTheDocument();
  expect([...document.querySelectorAll(".diff-code-delete, .diff-code-insert")].map((cell) => cell.textContent)).toEqual(["oldA", "oldB", "newA", "newB"]);
  expect(screen.queryByRole("button", { name: "Side by side" })).not.toBeInTheDocument();
  expect(document.querySelector(".diff-split")).not.toBeInTheDocument();
  expect(document.querySelector(".diff-unified")).toBeInTheDocument();
});

test("shows new-file line numbers on inserts and none on deletes", () => {
  render(<View />);
  const deleteGutters = [...document.querySelectorAll(".diff-gutter-delete")];
  expect(deleteGutters[1].textContent).toBe("");
  expect(deleteGutters[3].textContent).toBe("");
  const insertGutters = [...document.querySelectorAll(".diff-gutter-insert")];
  expect(insertGutters[1].textContent).toBe("2");
});

test("maps staged, unstaged, and discard hunk actions to backend-owned ids", async () => {
  const onRun = vi.fn();
  const onHunkSettled = vi.fn();
  const { rerender } = render(<View onRun={onRun} onHunkSettled={onHunkSettled} />);
  expect(screen.getByRole("button", { name: "Discard hunk" })).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Stage hunk" }));
  expect(onRun).toHaveBeenLastCalledWith({ type: "stageHunk", snapshotId: 7, hunkId: "h1" }, expect.any(Function));
  onRun.mock.calls.at(-1)?.[1]("succeeded");
  await waitFor(() => expect(onHunkSettled).toHaveBeenCalled());
  fireEvent.click(screen.getByRole("button", { name: "Discard hunk" }));
  expect(onRun).toHaveBeenLastCalledWith({ type: "discardHunk", snapshotId: 7, hunkId: "h1" }, expect.any(Function));
  onRun.mock.calls.at(-1)?.[1]("succeeded");
  await waitFor(() => expect(screen.getByRole("button", { name: "Stage hunk" })).toBeEnabled());
  rerender(<View value={{ ...diff, staged: true }} onRun={onRun} />);
  expect(screen.queryByRole("button", { name: "Discard hunk" })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Unstage hunk" }));
  expect(onRun).toHaveBeenLastCalledWith({ type: "unstageHunk", snapshotId: 7, hunkId: "h1" }, expect.any(Function));
});

test("stages each change island inside one git hunk", async () => {
  const onRun = vi.fn();
  const filePatch = "diff --git a/src/file.ts b/src/file.ts\n--- a/src/file.ts\n+++ b/src/file.ts\n@@ -1,5 +1,5 @@\n context\n-oldA\n+newA\n mid\n-oldB\n+newB\n";
  render(<View onRun={onRun} value={{
    path: "src/file.ts",
    staged: false,
    binary: false,
    tooLarge: false,
    patch: filePatch,
    hunks: [
      { id: "h1", header: "@@ -1,3 +1,3 @@", patch: "diff --git a/src/file.ts b/src/file.ts\n--- a/src/file.ts\n+++ b/src/file.ts\n@@ -1,3 +1,3 @@\n context\n-oldA\n+newA\n mid\n" },
      { id: "h2", header: "@@ -3,2 +3,2 @@", patch: "diff --git a/src/file.ts b/src/file.ts\n--- a/src/file.ts\n+++ b/src/file.ts\n@@ -3,2 +3,2 @@\n mid\n-oldB\n+newB\n" },
    ],
  }} />);
  const buttons = screen.getAllByRole("button", { name: "Stage hunk" });
  expect(buttons).toHaveLength(2);
  expect(screen.getAllByRole("button", { name: "Discard hunk" })).toHaveLength(2);
  fireEvent.click(buttons[0]);
  expect(onRun).toHaveBeenLastCalledWith({ type: "stageHunk", snapshotId: 7, hunkId: "h1" }, expect.any(Function));
  onRun.mock.calls.at(-1)?.[1]("succeeded");
  await waitFor(() => expect(screen.getAllByRole("button", { name: "Stage hunk" })[1]).toBeEnabled());
  fireEvent.click(screen.getAllByRole("button", { name: "Stage hunk" })[1]);
  expect(onRun).toHaveBeenLastCalledWith({ type: "stageHunk", snapshotId: 7, hunkId: "h2" }, expect.any(Function));
});

test("stacks staged and unstaged sections for a mixed file", () => {
  const staged = { ...diff, staged: true };
  const unstaged = { ...diff, staged: false, hunks: [{ id: "h2", header: "@@ -2,3 +2,2 @@", patch }] };
  render(<View value={staged} companion={unstaged} />);
  expect(screen.getByText("INDEX ↔ HEAD")).toBeInTheDocument();
  expect(screen.getByText("WORKTREE ↔ INDEX")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Unstage hunk" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Stage hunk" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Discard hunk" })).toBeInTheDocument();
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


test("branch comparison selects every file, including metadata and binary changes, without write actions", () => {
  const comparison = { patch: "merge base abc\n\n" + patch + "\n" + [
    "diff --git a/new.ts b/new.ts\nnew file mode 100644\n--- /dev/null\n+++ b/new.ts\n@@ -0,0 +1 @@\n+const created = 42;\n",
    "diff --git a/deleted.txt b/deleted.txt\ndeleted file mode 100644\n--- a/deleted.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\n",
    "diff --git a/old.txt b/renamed.txt\nsimilarity index 100%\nrename from old.txt\nrename to renamed.txt\n",
    "diff --git a/image.png b/image.png\nindex 1111111..2222222 100644\nBinary files a/image.png and b/image.png differ\n",
    "diff --git a/script.sh b/script.sh\nold mode 100644\nnew mode 100755\n",
  ].join("") };
  const onBack = vi.fn();
  const view = (value: { patch: string }) => <I18nProvider language="en"><BranchComparisonView comparison={value} onBack={onBack} /></I18nProvider>;
  const { rerender } = render(view(comparison));
  expect(screen.getByText("Changed files")).toBeInTheDocument();
  expect(document.querySelector(".commit-detail-files .pane-title code")).toHaveTextContent("6");
  expect(document.querySelector(".diff-lines")).not.toBeInTheDocument();
  const rows = [...document.querySelectorAll(".commit-file-row")];
  expect(rows[0]).toHaveTextContent("file.ts");
  expect(rows[0]).toHaveTextContent("src/file.ts");
  expect(rows[0].querySelector(".commit-stat-add")).toHaveTextContent("+1");
  expect(rows[0].querySelector(".commit-stat-del")).toHaveTextContent("−2");
  expect(rows[1].querySelector(".commit-file-stats")).toHaveTextContent("+1−0");
  expect(rows[2].querySelector(".commit-file-stats")).toHaveTextContent("+0−1");
  expect(rows[3]).toHaveTextContent("old.txt → renamed.txt");
  expect(rows[3].querySelector(".commit-file-stats")).toHaveTextContent("+0−0");
  expect(rows[4].querySelector(".commit-file-binary")).toBeInTheDocument();
  expect(rows[5].querySelector(".commit-file-stats")).toHaveTextContent("+0−0");
  const back = () => fireEvent.click(screen.getByRole("button", { name: /Back/ }));
  const open = (path: string) => fireEvent.click(screen.getByRole("button", { name: (name) => name.includes(path) }));
  open("new.ts");
  expect(document.querySelector(".commit-file-list")).not.toBeInTheDocument();
  expect(document.querySelector(".diff-code-insert")).toHaveTextContent("const created = 42;");
  expect(document.querySelector(".token.keyword")).toBeInTheDocument();
  back();
  expect(onBack).not.toHaveBeenCalled();
  open("deleted.txt");
  expect(document.querySelector(".diff-code-delete")).toHaveTextContent("gone");
  back();
  open("renamed.txt");
  expect(screen.getByText("rename from old.txt")).toBeInTheDocument();
  back();
  open("image.png");
  expect(screen.getByRole("heading", { name: "Binary diff" })).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /difftool/ })).not.toBeInTheDocument();
  back();
  open("script.sh");
  expect(screen.getByText("new mode 100755")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /hunk/ })).not.toBeInTheDocument();
  rerender(view({ ...comparison }));
  expect(document.querySelector(".diff-lines")).not.toBeInTheDocument();
  expect(document.querySelectorAll(".commit-file-row")).toHaveLength(6);
  back();
  expect(onBack).toHaveBeenCalledTimes(1);
  rerender(view({ patch: "merge base abc\n\n" }));
  expect(screen.getByRole("status")).toHaveTextContent("No differences");
});
