import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import type { FileChange, RepositorySummary, WorkingTreeSnapshot } from "../api";
import { I18nProvider } from "../i18n";
import { ChangesPane } from "./ChangesPane";

afterEach(cleanup);

test.each([
  { group: "Staged", staged: true, unstaged: false, kind: "modified" },
  { group: "Partially staged", staged: true, unstaged: true, kind: "modified" },
  { group: "Unstaged", staged: false, unstaged: true, kind: "modified" },
  { group: "Untracked", staged: false, unstaged: true, kind: "untracked" },
] as const)("shows partial selection in the $group checkbox", ({ group, staged, unstaged, kind }) => {
  const files: FileChange[] = ["one.ts", "two.ts"].map((path) => ({ path, originalPath: null, kind, staged, unstaged, indexStatus: null, worktreeStatus: null, conflict: false, ignored: false, additions: null, deletions: null }));
  render(<ChangesPane repository={{ id: 1 } as RepositorySummary} snapshot={{ id: 1, repositoryId: 1, headOid: null, files }} onOpen={vi.fn()} onOpenExternal={vi.fn()} onRun={vi.fn()} onFileHistory={vi.fn()} onBlame={vi.fn()} />);
  const groupCheckbox = screen.getByRole("checkbox", { name: `Select all ${group}` });
  const allCheckbox = screen.getByRole("checkbox", { name: "Select all Working tree" });
  expect(groupCheckbox).not.toBeChecked();
  expect(groupCheckbox).not.toBePartiallyChecked();
  fireEvent.click(screen.getByRole("checkbox", { name: /Select file .*one\.ts/ }));
  expect(groupCheckbox).toBePartiallyChecked();
  expect(allCheckbox).toBePartiallyChecked();
  fireEvent.click(screen.getByRole("button", { name: `${group}2` }));
  expect(groupCheckbox).toBePartiallyChecked();
  fireEvent.click(groupCheckbox);
  expect(groupCheckbox).toBeChecked();
  expect(groupCheckbox).not.toBePartiallyChecked();
  expect(allCheckbox).toBeChecked();
  fireEvent.click(groupCheckbox);
  expect(groupCheckbox).not.toBeChecked();
  expect(groupCheckbox).not.toBePartiallyChecked();
  expect(allCheckbox).not.toBeChecked();
  fireEvent.click(allCheckbox);
  expect(groupCheckbox).toBeChecked();
  fireEvent.click(allCheckbox);
  expect(groupCheckbox).not.toBeChecked();
  expect(groupCheckbox).not.toBePartiallyChecked();
});

test("updates select-all after snapshots change and disables it without selectable files", () => {
  const repository = { id: 1 } as RepositorySummary;
  const file: FileChange = { path: "one.ts", originalPath: null, kind: "modified", indexStatus: null, worktreeStatus: "M", staged: false, unstaged: true, conflict: false, ignored: false, additions: null, deletions: null };
  const callbacks = { onOpen: vi.fn(), onOpenExternal: vi.fn(), onRun: vi.fn(), onFileHistory: vi.fn(), onBlame: vi.fn() };
  const pane = (snapshot?: WorkingTreeSnapshot, selectedRepository?: RepositorySummary) => <ChangesPane {...callbacks} repository={selectedRepository} snapshot={snapshot} />;
  const { rerender } = render(pane());
  const selectAll = () => screen.getByRole("checkbox", { name: "Select all Working tree" });
  expect(selectAll()).toBeDisabled();
  rerender(pane(undefined, repository));
  expect(selectAll()).toBeDisabled();

  const snapshot = { id: 1, repositoryId: 1, headOid: null, files: [file] };
  rerender(pane(snapshot, repository));
  fireEvent.click(selectAll());
  expect(selectAll()).toBeChecked();
  rerender(pane({ ...snapshot, id: 2, files: [file, { ...file, path: "two.ts" }] }, repository));
  expect(selectAll()).toBePartiallyChecked();
  expect(screen.getByRole("checkbox", { name: "Select all Unstaged" })).toBePartiallyChecked();
  rerender(pane({ ...snapshot, id: 3, files: [{ ...file, path: "two.ts" }] }, repository));
  expect(selectAll()).not.toBeChecked();
  expect(selectAll()).not.toBePartiallyChecked();
  expect(screen.getByRole("checkbox", { name: "Select all Unstaged" })).not.toBePartiallyChecked();
  expect(screen.queryByRole("button", { name: /Stage selected/ })).not.toBeInTheDocument();

  rerender(pane({ ...snapshot, id: 4, files: [] }, repository));
  expect(selectAll()).toBeDisabled();
  expect(selectAll()).not.toBeChecked();
  rerender(pane({ ...snapshot, id: 5, files: [
    { ...file, path: "conflict.ts", kind: "conflicted", conflict: true },
    { ...file, path: "ignored.log", kind: "ignored", ignored: true, unstaged: false },
  ] }, repository));
  expect(selectAll()).toBeDisabled();
  expect(selectAll()).not.toBePartiallyChecked();

  rerender(<I18nProvider language="zh-CN">{pane(snapshot, repository)}</I18nProvider>);
  expect(screen.getByRole("checkbox", { name: "全选 工作区" })).toBeEnabled();
});
