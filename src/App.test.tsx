import "@testing-library/jest-dom/vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import { afterEach, expect, test, vi } from "vitest";
import App from "./App";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn((command: string) => command === "bootstrap" ? Promise.resolve({ git: { supported: true, version: "2.50.1", path: "/usr/bin/git" }, settings: {}, repositories: [] }) : Promise.resolve([])) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: vi.fn(() => ({ onCloseRequested: vi.fn(() => Promise.resolve(() => {})), close: vi.fn() })) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(() => Promise.resolve(null)) }));

const dispatchToggle = (element: HTMLElement, newState: "closed" | "open") => {
  const event = new Event("toggle");
  Object.defineProperty(event, "newState", { value: newState });
  element.dispatchEvent(event);
};
const showPopover = vi.fn(function (this: HTMLElement) { dispatchToggle(this, "open"); });
const hidePopover = vi.fn(function (this: HTMLElement) { dispatchToggle(this, "closed"); });
Object.defineProperties(HTMLElement.prototype, {
  showPopover: { configurable: true, value: showPopover },
  hidePopover: { configurable: true, value: hidePopover },
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

test("shows actionable first-run state", async () => {
  render(<App />);
  expect(await screen.findByText(/Put every working tree/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Add repository" })).toBeEnabled();
  const contextMenu = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
  window.dispatchEvent(contextMenu);
  expect(contextMenu.defaultPrevented).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Initialize" }));
});

test("creates a branch from the inline form", async () => {
  const prompt = vi.spyOn(window, "prompt");
  vi.mocked(invoke).mockImplementation((command: string) => {
    if (command === "bootstrap") return Promise.resolve({
      git: { supported: true, version: "2.50.1", path: "/usr/bin/git" },
      settings: { selectedRepositoryId: 1, leftWidth: 240, rightWidth: 360, outputHeight: 190 },
      repositories: [{ id: 1, path: "/repo", name: "Repo", favorite: false, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "12345678", changedCount: 0, conflictCount: 0, ahead: 0, behind: 0 }],
    });
    if (command === "get_status") return Promise.resolve({ id: 1, repositoryId: 1, headOid: "12345678", files: [] });
    if (command === "get_branches") return Promise.resolve([
      { name: "main", oid: "12345678", current: true, remote: false },
      { name: "origin/main", oid: "12345678", current: false, remote: true },
    ]);
    if (["get_tags", "get_remotes", "get_submodules"].includes(command)) return Promise.resolve([]);
    if (command === "preview_operation") return Promise.resolve({ title: "Create branch", summary: "", risk: "normal", affectedPaths: [], affectedRefs: [], recoverable: true, requiresConfirmation: false });
    if (command === "start_operation") return Promise.resolve({ operationId: 1, accepted: true });
    return Promise.resolve(undefined);
  });

  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Branches" }));
  const newBranch = await screen.findByRole("button", { name: "New branch" });
  expect(screen.getByText("Local branches")).toBeInTheDocument();
  expect(screen.getByText("Remote branches")).toBeInTheDocument();
  fireEvent.click(newBranch);
  const input = screen.getByRole("textbox", { name: "New branch name" });
  expect(input).toHaveFocus();
  expect(screen.getByRole("button", { name: "Create" })).toBeDisabled();
  expect(screen.getAllByRole("button", { name: "More actions" })[0]).toHaveAttribute("aria-haspopup", "menu");
  expect(document.querySelector("[popover='auto']")).toBeInTheDocument();
  fireEvent.keyDown(input, { key: "Escape" });
  expect(screen.queryByRole("textbox", { name: "New branch name" })).not.toBeInTheDocument();

  fireEvent.click(newBranch);
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(screen.queryByRole("textbox", { name: "New branch name" })).not.toBeInTheDocument();

  fireEvent.click(newBranch);
  fireEvent.change(screen.getByRole("textbox", { name: "New branch name" }), { target: { value: " feature/test " } });
  fireEvent.click(screen.getByRole("button", { name: "Create" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("preview_operation", { repositoryId: 1, request: { type: "createBranch", name: "feature/test", checkout: true } }));
  expect(prompt).not.toHaveBeenCalled();
});

test("renders history topology and ref labels", async () => {
  vi.mocked(invoke).mockImplementation((command: string) => {
    if (command === "bootstrap") return Promise.resolve({
      git: { supported: true, version: "2.50.1", path: "/usr/bin/git" },
      settings: { selectedRepositoryId: 1, leftWidth: 240, rightWidth: 360, outputHeight: 190 },
      repositories: [{ id: 1, path: "/repo", name: "Repo", favorite: false, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "aaaaaaaa", changedCount: 0, conflictCount: 0, ahead: 0, behind: 0 }],
    });
    if (command === "get_status") return Promise.resolve({ id: 1, repositoryId: 1, headOid: "aaaaaaaa", files: [] });
    if (command === "get_history") return Promise.resolve({ commits: [
      { oid: "aaaaaaaa", parents: ["bbbbbbbb", "cccccccc"], author: "Ada", authoredAt: "2026-08-09T00:00:00Z", subject: "Merge feature", refs: ["HEAD -> main", "tag: v1.0"], lane: { column: 0, parentColumns: [0, 2] } },
      { oid: "bbbbbbbb", parents: ["dddddddd"], author: "Ada", authoredAt: "2026-08-08T00:00:00Z", subject: "Main work", refs: [], lane: { column: 0, parentColumns: [0] } },
      { oid: "cccccccc", parents: ["dddddddd"], author: "Lin", authoredAt: "2026-08-08T00:00:00Z", subject: "Feature work", refs: ["origin/feature"], lane: { column: 2, parentColumns: [0] } },
      { oid: "dddddddd", parents: [], author: "Ada", authoredAt: "2026-08-07T00:00:00Z", subject: "Base", refs: [], lane: { column: 0, parentColumns: [] } },
    ], nextOffset: undefined });
    return Promise.resolve([]);
  });

  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "History" }));
  await screen.findByText("HEAD -> main");
  expect(screen.getByText("tag: v1.0")).toHaveClass("tag");
  expect(document.querySelectorAll(".graph-node")).toHaveLength(4);
  expect(document.querySelectorAll(".graph-edge")).toHaveLength(4);
  expect(document.querySelector<HTMLElement>(".graph-list")?.style.getPropertyValue("--graph-width")).toBe("52px");
  expect(screen.getByRole("button", { name: /Git output/ })).toHaveClass("output-handle");
});

test("loads more history without losing the selected commit", async () => {
  vi.mocked(invoke).mockImplementation((command: string, args?: Parameters<typeof invoke>[1]) => {
    if (command === "bootstrap") return Promise.resolve({
      git: { supported: true, version: "2.50.1", path: "/usr/bin/git" },
      settings: { selectedRepositoryId: 1, leftWidth: 240, rightWidth: 360, outputHeight: 190 },
      repositories: [{ id: 1, path: "/repo", name: "Repo", favorite: false, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "aaaaaaaa", changedCount: 0, conflictCount: 0, ahead: 0, behind: 0 }],
    });
    if (command === "get_status") return Promise.resolve({ id: 1, repositoryId: 1, headOid: "aaaaaaaa", files: [] });
    if (command === "get_history" && args && "cursor" in args && args.cursor && (args.cursor as { offset: number }).offset === 100) return Promise.resolve({ commits: [{ oid: "bbbbbbbb", parents: [], author: "Lin", authoredAt: "2026-08-08T00:00:00Z", subject: "Older", refs: [], lane: { column: 0, parentColumns: [] } }], nextCursor: undefined });
    if (command === "get_history") return Promise.resolve({ commits: [{ oid: "aaaaaaaa", parents: [], author: "Ada", authoredAt: "2026-08-09T00:00:00Z", subject: "Selected", refs: [], lane: { column: 0, parentColumns: [] } }], nextCursor: { offset: 100, activeLanes: [] } });
    if (command === "get_commit_diff") return Promise.resolve("diff --git a/a b/a");
    return Promise.resolve([]);
  });

  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "History" }));
  fireEvent.click((await screen.findAllByRole("button", { name: /Selected/ }))[0]);
  fireEvent.click(await screen.findByRole("button", { name: /Back/ }));
  fireEvent.click(screen.getByRole("button", { name: "Load more" }));
  expect((await screen.findAllByText("Older")).length).toBeGreaterThan(0);
  expect(screen.getAllByRole("button", { name: /Selected/ }).some((button) => button.classList.contains("selected") || button.parentElement?.classList.contains("selected"))).toBe(true);
  expect(invoke).toHaveBeenCalledWith("get_history", { repositoryId: 1, cursor: { offset: 100, activeLanes: [] }, limit: 100 });
});

test("hides pagination when switching to a repository without another page", async () => {
  const repositories = [
    { id: 1, path: "/large", name: "Large", favorite: false, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "aaaaaaaa", changedCount: 0, conflictCount: 0, ahead: 0, behind: 0 },
    { id: 2, path: "/small", name: "Small", favorite: false, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "bbbbbbbb", changedCount: 0, conflictCount: 0, ahead: 0, behind: 0 },
  ];
  vi.mocked(invoke).mockImplementation((command: string, args?: Parameters<typeof invoke>[1]) => {
    if (command === "bootstrap") return Promise.resolve({ git: { supported: true, version: "2.50.1", path: "/usr/bin/git" }, settings: { selectedRepositoryId: 1, leftWidth: 240, rightWidth: 360, outputHeight: 190 }, repositories });
    if (command === "get_status") return Promise.resolve({ id: 1, repositoryId: 1, headOid: "aaaaaaaa", files: [] });
    if (command === "get_history" && args && "repositoryId" in args && args.repositoryId === 2) return Promise.resolve({ commits: [{ oid: "bbbbbbbb", parents: [], author: "Lin", authoredAt: "2026-08-08T00:00:00Z", subject: "Small history", refs: [], lane: { column: 0, parentColumns: [] } }], nextCursor: null });
    if (command === "get_history") return Promise.resolve({ commits: [{ oid: "aaaaaaaa", parents: [], author: "Ada", authoredAt: "2026-08-09T00:00:00Z", subject: "Large history", refs: [], lane: { column: 0, parentColumns: [] } }], nextCursor: { offset: 100, activeLanes: [] } });
    return Promise.resolve([]);
  });

  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "History" }));
  expect(await screen.findByRole("button", { name: "Load more" })).toBeInTheDocument();
  fireEvent.click(screen.getByRole("option", { name: /Small/ }));
  expect(screen.queryByRole("button", { name: "Load more" })).not.toBeInTheDocument();
  expect((await screen.findAllByText("Small history")).length).toBeGreaterThan(0);
  expect(screen.queryByRole("button", { name: "Load more" })).not.toBeInTheDocument();
  expect(invoke).not.toHaveBeenCalledWith("get_history", expect.objectContaining({ cursor: null }));
});

test("reloads history after leaving during the initial request", async () => {
  let historyCalls = 0;
  vi.mocked(invoke).mockImplementation((command: string) => {
    if (command === "bootstrap") return Promise.resolve({
      git: { supported: true, version: "2.50.1", path: "/usr/bin/git" },
      settings: { selectedRepositoryId: 1, leftWidth: 240, rightWidth: 360, outputHeight: 190 },
      repositories: [{ id: 1, path: "/repo", name: "Repo", favorite: false, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "aaaaaaaa", changedCount: 0, conflictCount: 0, ahead: 0, behind: 0 }],
    });
    if (command === "get_status") return Promise.resolve({ id: 1, repositoryId: 1, headOid: "aaaaaaaa", files: [] });
    if (command === "get_history") {
      historyCalls += 1;
      if (historyCalls === 1) return new Promise(() => {});
      return Promise.resolve({ commits: [{ oid: "bbbbbbbb", parents: [], author: "Lin", authoredAt: "2026-08-08T00:00:00Z", subject: "Reloaded", refs: [], lane: { column: 0, parentColumns: [] } }], nextOffset: undefined });
    }
    return Promise.resolve([]);
  });

  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "History" }));
  await waitFor(() => expect(historyCalls).toBe(1));
  fireEvent.click(screen.getByRole("button", { name: "Changes" }));
  fireEvent.click(screen.getByRole("button", { name: "History" }));
  expect((await screen.findAllByText("Reloaded")).length).toBeGreaterThan(0);
  expect(historyCalls).toBe(2);
});

test("starts clone from a validated in-app form", async () => {
  let operationListener: ((event: { payload: { operationId: number; repositoryId?: number | null; kind: "started" | "stderr" | "finished"; message: string; outcome?: "failed" } }) => void) | undefined;
  let resolveClone: ((result: { operationId: number; accepted: boolean }) => void) | undefined;
  vi.mocked(listen).mockImplementation(((event: string, handler: typeof operationListener) => {
    if (event === "operation-event") operationListener = handler;
    return Promise.resolve(() => {});
  }) as never);
  vi.mocked(open).mockResolvedValue("/tmp/clone");
  vi.mocked(invoke).mockImplementation((command: string) => {
    if (command === "bootstrap") return Promise.resolve({ git: { supported: true, version: "2.50.1", path: "/usr/bin/git" }, settings: {}, repositories: [] });
    if (command === "clone_repository") return new Promise((resolve) => { resolveClone = resolve; });
    return Promise.resolve([]);
  });
  const prompt = vi.spyOn(window, "prompt");
  const confirm = vi.spyOn(window, "confirm");

  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Clone" }));
  const input = screen.getByRole("textbox", { name: "Remote URL" });
  const submit = screen.getByRole("dialog").querySelector<HTMLButtonElement>("button[type='submit']")!;
  expect(submit).toBeDisabled();
  fireEvent.change(input, { target: { value: " https://example.com/repo.git " } });
  fireEvent.click(submit);
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("clone_repository", { url: "https://example.com/repo.git", destination: "/tmp/clone" }));
  expect(screen.getByRole("button", { name: /Git output/ })).toBeInTheDocument();
  await act(async () => operationListener?.({ payload: { operationId: 7, repositoryId: null, kind: "started", message: "Clone repository" } }));
  await act(async () => operationListener?.({ payload: { operationId: 7, kind: "stderr", message: "Receiving objects" } }));
  expect(screen.getByRole("button", { name: "Cancel #7" })).toBeInTheDocument();
  await act(async () => operationListener?.({ payload: { operationId: 7, kind: "finished", message: "Clone failed", outcome: "failed" } }));
  await act(async () => resolveClone?.({ operationId: 7, accepted: true }));
  expect(screen.queryByRole("button", { name: "Cancel #7" })).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: /Git output/ })).toHaveTextContent("3 lines");
  expect(prompt).not.toHaveBeenCalled();
  expect(confirm).not.toHaveBeenCalled();
});

test("repository list refresh preserves the current selection", async () => {
  let repositoryListListener: (() => void) | undefined;
  const repositories = [
    { id: 1, path: "/alpha", name: "Alpha", favorite: false, order: 0, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "aaaaaaaa", changedCount: 0, conflictCount: 0, ahead: 0, behind: 0 },
    { id: 2, path: "/beta", name: "Beta", favorite: false, order: 1, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "bbbbbbbb", changedCount: 0, conflictCount: 0, ahead: 0, behind: 0 },
  ];
  let repositorySummaryListener: ((event: { payload: (typeof repositories)[number] }) => void) | undefined;
  vi.mocked(listen).mockImplementation(((event: string, handler: (...args: never[]) => void) => {
    if (event === "repository-list-changed") repositoryListListener = handler;
    if (event === "repository-summary-refreshed") repositorySummaryListener = handler as typeof repositorySummaryListener;
    return Promise.resolve(() => {});
  }) as never);
  vi.mocked(invoke).mockImplementation((command: string) => {
    if (command === "bootstrap") return Promise.resolve({ git: { supported: true, version: "2.50.1", path: "/usr/bin/git" }, settings: { selectedRepositoryId: 2, leftWidth: 240, rightWidth: 360, outputHeight: 190 }, repositories });
    if (command === "refresh_repositories") return Promise.resolve(repositories);
    if (command === "get_status") return Promise.resolve({ id: 1, repositoryId: 1, headOid: "aaaaaaaa", files: [] });
    return Promise.resolve([]);
  });

  render(<App />);
  const alpha = await screen.findByRole("option", { name: /Alpha/ });
  fireEvent.click(alpha);
  expect(alpha).toHaveAttribute("aria-selected", "true");
  await act(async () => repositorySummaryListener?.({ payload: { ...repositories[0], changedCount: 3 } }));
  expect(screen.getByRole("option", { name: /Alpha/ })).toHaveTextContent("±3");
  await act(async () => repositoryListListener?.());
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("refresh_repositories", { activeRepositoryId: 1 }));
  expect(screen.getByRole("option", { name: /Alpha/ })).toHaveAttribute("aria-selected", "true");
});

test("uses light-dismiss popovers for secondary menus", async () => {
  vi.mocked(invoke).mockImplementation((command: string) => {
    if (command === "bootstrap") return Promise.resolve({
      git: { supported: true, version: "2.50.1", path: "/usr/bin/git" },
      settings: { selectedRepositoryId: 1, leftWidth: 240, rightWidth: 360, outputHeight: 190 },
      repositories: [{ id: 1, path: "/repo", name: "Repo", favorite: false, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "12345678", changedCount: 0, conflictCount: 0, ahead: 0, behind: 0 }],
    });
    if (command === "get_status") return Promise.resolve({ id: 1, repositoryId: 1, headOid: "12345678", files: [] });
    if (command === "refresh_repositories") return Promise.resolve([]);
    return Promise.resolve(undefined);
  });

  render(<App />);
  const more = await screen.findByRole("button", { name: "More" });
  const menu = more.nextElementSibling as HTMLDivElement;
  Object.defineProperty(menu, "scrollHeight", { value: 120 });
  expect(document.querySelector("details")).not.toBeInTheDocument();
  expect(menu).toHaveAttribute("popover", "auto");
  expect(more).toHaveAttribute("aria-expanded", "false");

  fireEvent.click(more);
  expect(showPopover).toHaveBeenCalledWith();
  expect(menu.style.height).toBe("122px");
  expect(more).toHaveAttribute("aria-expanded", "true");
  fireEvent.click(menu.querySelector("button")!);
  expect(hidePopover).toHaveBeenCalled();
  expect(more).toHaveAttribute("aria-expanded", "false");

  fireEvent.click(more);
  hidePopover.mockClear();
  const querySelector = vi.spyOn(document, "querySelector").mockReturnValue(menu);
  fireEvent(window, new Event("blur"));
  querySelector.mockRestore();
  expect(hidePopover).toHaveBeenCalledOnce();
  expect(more).toHaveAttribute("aria-expanded", "false");
});

test("switches language and persists the choice", async () => {
  vi.mocked(invoke).mockImplementation((command: string) => command === "bootstrap" ? Promise.resolve({
    git: { supported: true, version: "2.50.1", path: "/usr/bin/git" },
    settings: { language: "zh-CN" }, repositories: [],
  }) : Promise.resolve(undefined));

  render(<App />);
  expect(await screen.findByRole("button", { name: "添加仓库" })).toBeEnabled();
  fireEvent.click(screen.getByRole("button", { name: "EN" }));
  expect(screen.getByRole("button", { name: "Add repository" })).toBeEnabled();
  expect(document.documentElement.lang).toBe("en");
  expect(invoke).toHaveBeenCalledWith("save_language", { language: "en" });
});

test("stages and unstages multiple selected files", async () => {
  vi.mocked(invoke).mockImplementation((command: string) => {
    if (command === "bootstrap") return Promise.resolve({
      git: { supported: true, version: "2.50.1", path: "/usr/bin/git" },
      settings: { selectedRepositoryId: 1, leftWidth: 240, rightWidth: 360, outputHeight: 190, language: "en" },
      repositories: [{ id: 1, path: "/repo", name: "Repo", favorite: false, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "12345678", changedCount: 6, conflictCount: 1, ahead: 0, behind: 0 }],
    });
    if (command === "get_status") return Promise.resolve({ id: 1, repositoryId: 1, headOid: "12345678", files: [
      { path: "staged.ts", kind: "Modified", staged: true, unstaged: false, conflict: false, ignored: false },
      { path: "one.ts", kind: "Modified", staged: false, unstaged: true, conflict: false, ignored: false },
      { path: "two.ts", kind: "Modified", staged: false, unstaged: true, conflict: false, ignored: false },
      { path: "new.ts", kind: "Untracked", staged: false, unstaged: true, conflict: false, ignored: false },
      { path: "conflict.ts", kind: "Conflicted", staged: false, unstaged: true, conflict: true, ignored: false },
      { path: "ignored.log", kind: "Ignored", staged: false, unstaged: false, conflict: false, ignored: true },
    ] });
    if (command === "preview_operation") return Promise.resolve({ title: "Operation", summary: "", risk: "normal", affectedPaths: [], affectedRefs: [], recoverable: true, requiresConfirmation: false });
    if (command === "start_operation") return Promise.resolve({ operationId: 1, accepted: true });
    return Promise.resolve(undefined);
  });

  render(<App />);
  fireEvent.click(await screen.findByRole("checkbox", { name: "Select all Unstaged" }));
  fireEvent.click(screen.getByRole("checkbox", { name: "Select file to stage new.ts" }));
  fireEvent.click(screen.getByRole("button", { name: "Stage selected (3)" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("preview_operation", { repositoryId: 1, request: { type: "stageFiles", paths: ["one.ts", "two.ts", "new.ts"] } }));

  fireEvent.click(screen.getByRole("checkbox", { name: "Select file to unstage staged.ts" }));
  fireEvent.click(screen.getByRole("button", { name: "Unstage selected (1)" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("preview_operation", { repositoryId: 1, request: { type: "unstageFiles", paths: ["staged.ts"] } }));
  expect(screen.queryByRole("checkbox", { name: /conflict\.ts/ })).not.toBeInTheDocument();
  expect(screen.queryByRole("checkbox", { name: /ignored\.log/ })).not.toBeInTheDocument();
});

test("routes conflict actions through previews and shows destructive impact", async () => {
  vi.mocked(invoke).mockImplementation((command: string, args?: Parameters<typeof invoke>[1]) => {
    if (command === "bootstrap") return Promise.resolve({
      git: { supported: true, version: "2.50.1", path: "/usr/bin/git" }, settings: { selectedRepositoryId: 1, leftWidth: 240, rightWidth: 360, outputHeight: 190 },
      repositories: [{ id: 1, path: "/repo", name: "Repo", favorite: false, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "12345678", changedCount: 2, conflictCount: 1, ahead: 0, behind: 0 }],
    });
    if (command === "get_status") return Promise.resolve({ id: 1, repositoryId: 1, headOid: "12345678", files: [
      { path: "conflict.ts", kind: "Conflicted", staged: false, unstaged: true, conflict: true, ignored: false },
      { path: "work.ts", kind: "Modified", staged: false, unstaged: true, conflict: false, ignored: false },
    ] });
    if (command === "preview_operation" && args && "request" in args && (args.request as { type: string }).type === "discardTracked") return Promise.resolve({ title: "Discard tracked changes", summary: "", risk: "destructive", affectedPaths: ["work.ts"], affectedRefs: [], recoverable: false, requiresConfirmation: true });
    if (command === "preview_operation") return Promise.resolve({ title: "Resolve", summary: "", risk: "normal", affectedPaths: [], affectedRefs: [], recoverable: true, requiresConfirmation: false });
    if (command === "start_operation") return Promise.resolve({ operationId: 2, accepted: true });
    return Promise.resolve([]);
  });

  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Resolve" }));
  fireEvent.click(screen.getByRole("button", { name: "Use current target", hidden: true }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_operation", { repositoryId: 1, request: { type: "chooseConflictSide", path: "conflict.ts", side: "ours" }, confirmed: false }));
  fireEvent.click(screen.getByRole("button", { name: "Discard work.ts" }));
  expect(await screen.findByRole("alertdialog")).toHaveTextContent("work.ts");
});

test("opens output on failure and confirms cancellation before close", async () => {
  let operationListener: ((event: { payload: { operationId: number; repositoryId?: number; kind: "started" | "finished"; message: string; exitCode?: number; outcome?: string } }) => void) | undefined;
  let closeListener: ((event: { preventDefault: () => void }) => void) | undefined;
  const close = vi.fn(() => Promise.resolve());
  vi.mocked(listen).mockImplementation(((event: string, handler: typeof operationListener) => {
    if (event === "operation-event") operationListener = handler;
    return Promise.resolve(() => {});
  }) as never);
  vi.mocked(getCurrentWindow).mockReturnValue({ onCloseRequested: vi.fn((handler) => { closeListener = handler as typeof closeListener; return Promise.resolve(() => {}); }), close } as never);
  vi.mocked(invoke).mockImplementation((command: string) => {
    if (command === "bootstrap") return Promise.resolve({
      git: { supported: true, version: "2.50.1", path: "/usr/bin/git" }, settings: { selectedRepositoryId: 1, leftWidth: 240, rightWidth: 360, outputHeight: 190 },
      repositories: [{ id: 1, path: "/repo", name: "Repo", favorite: false, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "12345678", changedCount: 0, conflictCount: 0, ahead: 0, behind: 0 }],
    });
    if (command === "get_status") return Promise.resolve({ id: 1, repositoryId: 1, headOid: "12345678", files: [] });
    return Promise.resolve(undefined);
  });

  render(<App />);
  await screen.findByRole("option");
  await act(async () => operationListener?.({ payload: { operationId: 9, repositoryId: 1, kind: "started", message: "Pull" } }));
  const preventDefault = vi.fn();
  await act(async () => closeListener?.({ preventDefault }));
  expect(preventDefault).toHaveBeenCalled();
  fireEvent.click(await screen.findByRole("button", { name: "Confirm" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("cancel_operation", { operationId: 9 }));
  expect(close).toHaveBeenCalled();

  await act(async () => operationListener?.({ payload: { operationId: 10, repositoryId: 1, kind: "finished", message: "Failed", exitCode: 1, outcome: "failed" } }));
  expect(document.querySelector(".output-panel")).toHaveClass("open");
});

test("clears the commit message only after a successful early completion event", async () => {
  let operationListener: ((event: { payload: { operationId: number; repositoryId: number; kind: "started" | "finished"; message: string; outcome?: "succeeded" } }) => void) | undefined;
  let resolveStart: ((result: { operationId: number; accepted: boolean }) => void) | undefined;
  vi.mocked(listen).mockImplementation(((event: string, handler: typeof operationListener) => {
    if (event === "operation-event") operationListener = handler;
    return Promise.resolve(() => {});
  }) as never);
  vi.mocked(invoke).mockImplementation((command: string, args?: Parameters<typeof invoke>[1]) => {
    if (command === "bootstrap") return Promise.resolve({
      git: { supported: true, version: "2.50.1", path: "/usr/bin/git" }, settings: { selectedRepositoryId: 1, leftWidth: 240, rightWidth: 360, outputHeight: 190 },
      repositories: [
        { id: 1, path: "/repo", name: "Repo", favorite: false, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "12345678", changedCount: 1, conflictCount: 0, ahead: 0, behind: 0 },
        { id: 2, path: "/other", name: "Other", favorite: false, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "87654321", changedCount: 0, conflictCount: 0, ahead: 0, behind: 0 },
      ],
    });
    if (command === "get_status" && args && "repositoryId" in args && args.repositoryId === 2) return Promise.resolve({ id: 2, repositoryId: 2, headOid: "87654321", files: [] });
    if (command === "get_status") return Promise.resolve({ id: 1, repositoryId: 1, headOid: "12345678", files: [{ path: "file.ts", kind: "Modified", staged: true, unstaged: false, conflict: false, ignored: false }] });
    if (command === "preview_operation") return Promise.resolve({ title: "Commit", summary: "", risk: "normal", affectedPaths: [], affectedRefs: [], recoverable: true, requiresConfirmation: false });
    if (command === "start_operation") return new Promise((resolve) => { resolveStart = resolve; });
    return Promise.resolve(undefined);
  });

  render(<App />);
  const message = await screen.findByRole("textbox", { name: "Commit message" });
  fireEvent.change(message, { target: { value: "Ship it" } });
  fireEvent.click(screen.getByRole("button", { name: "Commit staged changes" }));
  expect(await screen.findByRole("button", { name: "running" })).toBeDisabled();
  await waitFor(() => expect(resolveStart).toBeDefined());
  await act(async () => operationListener?.({ payload: { operationId: 7, repositoryId: 1, kind: "started", message: "Commit" } }));
  await act(async () => operationListener?.({ payload: { operationId: 7, repositoryId: 1, kind: "finished", message: "Done", outcome: "succeeded" } }));
  expect(message).toHaveValue("Ship it");
  await act(async () => resolveStart?.({ operationId: 7, accepted: true }));
  expect(message).toHaveValue("");

  resolveStart = undefined;
  fireEvent.change(message, { target: { value: "First draft" } });
  fireEvent.click(screen.getByRole("button", { name: "Commit staged changes" }));
  await waitFor(() => expect(resolveStart).toBeDefined());
  await act(async () => operationListener?.({ payload: { operationId: 8, repositoryId: 1, kind: "started", message: "Commit" } }));
  fireEvent.change(message, { target: { value: "Next draft" } });
  await act(async () => operationListener?.({ payload: { operationId: 8, repositoryId: 1, kind: "finished", message: "Done", outcome: "succeeded" } }));
  expect(message).toHaveValue("Next draft");
  await act(async () => resolveStart?.({ operationId: 8, accepted: true }));

  resolveStart = undefined;
  fireEvent.change(message, { target: { value: "Switch draft" } });
  fireEvent.click(screen.getByRole("button", { name: "Commit staged changes" }));
  await waitFor(() => expect(resolveStart).toBeDefined());
  await act(async () => operationListener?.({ payload: { operationId: 9, repositoryId: 1, kind: "started", message: "Commit" } }));
  fireEvent.click(screen.getByRole("option", { name: /Other/ }));
  const otherMessage = await screen.findByRole("textbox", { name: "Commit message" });
  fireEvent.change(otherMessage, { target: { value: "Other draft" } });
  await act(async () => operationListener?.({ payload: { operationId: 9, repositoryId: 1, kind: "finished", message: "Done", outcome: "succeeded" } }));
  await act(async () => resolveStart?.({ operationId: 9, accepted: true }));
  expect(otherMessage).toHaveValue("Other draft");
  fireEvent.click(screen.getByRole("option", { name: /Repo/ }));
  expect(await screen.findByRole("textbox", { name: "Commit message" })).toHaveValue("");
});

test("keeps the commit message after failed and cancelled operations", async () => {
  let operationListener: ((event: { payload: { operationId: number; repositoryId: number; kind: "started" | "finished"; message: string; outcome?: "failed" | "cancelled" } }) => void) | undefined;
  let nextOperationId = 0;
  vi.mocked(listen).mockImplementation(((event: string, handler: typeof operationListener) => {
    if (event === "operation-event") operationListener = handler;
    return Promise.resolve(() => {});
  }) as never);
  vi.mocked(invoke).mockImplementation((command: string) => {
    if (command === "bootstrap") return Promise.resolve({
      git: { supported: true, version: "2.50.1", path: "/usr/bin/git" }, settings: { selectedRepositoryId: 1, leftWidth: 240, rightWidth: 360, outputHeight: 190 },
      repositories: [{ id: 1, path: "/repo", name: "Repo", favorite: false, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "12345678", changedCount: 1, conflictCount: 0, ahead: 0, behind: 0 }],
    });
    if (command === "get_status") return Promise.resolve({ id: 1, repositoryId: 1, headOid: "12345678", files: [{ path: "file.ts", kind: "Modified", staged: true, unstaged: false, conflict: false, ignored: false }] });
    if (command === "preview_operation") return Promise.resolve({ title: "Commit", summary: "", risk: "normal", affectedPaths: [], affectedRefs: [], recoverable: true, requiresConfirmation: false });
    if (command === "start_operation") return Promise.resolve({ operationId: ++nextOperationId, accepted: true });
    return Promise.resolve(undefined);
  });

  render(<App />);
  const message = await screen.findByRole("textbox", { name: "Commit message" });
  fireEvent.change(message, { target: { value: "Keep me" } });
  for (const outcome of ["failed", "cancelled"] as const) {
    fireEvent.click(screen.getByRole("button", { name: "Commit staged changes" }));
    await waitFor(() => expect(nextOperationId).toBe(outcome === "failed" ? 1 : 2));
    await act(async () => operationListener?.({ payload: { operationId: nextOperationId, repositoryId: 1, kind: "started", message: "Commit" } }));
    await act(async () => operationListener?.({ payload: { operationId: nextOperationId, repositoryId: 1, kind: "finished", message: outcome, outcome } }));
    expect(message).toHaveValue("Keep me");
    expect(screen.getByRole("button", { name: "Commit staged changes" })).toBeEnabled();
  }
});

test("refreshes only the changed repository and status for the selected repository", async () => {
  let repositoryChanged: ((event: { payload: { repositoryId: number } }) => void) | undefined;
  const repositories = [
    { id: 1, path: "/one", name: "One", favorite: false, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "11111111", changedCount: 0, conflictCount: 0, ahead: 0, behind: 0 },
    { id: 2, path: "/two", name: "Two", favorite: false, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "22222222", changedCount: 0, conflictCount: 0, ahead: 0, behind: 0 },
  ];
  vi.mocked(listen).mockImplementation(((event: string, handler: typeof repositoryChanged) => {
    if (event === "repository-changed") repositoryChanged = handler;
    return Promise.resolve(() => {});
  }) as never);
  vi.mocked(invoke).mockImplementation((command: string, args?: Parameters<typeof invoke>[1]) => {
    if (command === "bootstrap") return Promise.resolve({ git: { supported: true, version: "2.50.1", path: "/usr/bin/git" }, settings: { selectedRepositoryId: 1, leftWidth: 240, rightWidth: 360, outputHeight: 190 }, repositories });
    if (command === "get_status") return Promise.resolve({ id: 1, repositoryId: Number(args && "repositoryId" in args ? args.repositoryId : 1), headOid: "11111111", files: [] });
    if (command === "refresh_repository") return Promise.resolve(repositories[Number(args && "repositoryId" in args ? args.repositoryId : 1) - 1]);
    return Promise.resolve(undefined);
  });

  render(<App />);
  await screen.findByRole("option", { name: /One/ });
  vi.mocked(invoke).mockClear();
  await act(async () => repositoryChanged?.({ payload: { repositoryId: 2 } }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("refresh_repository", { repositoryId: 2 }));
  expect(invoke).not.toHaveBeenCalledWith("get_status", expect.objectContaining({ repositoryId: 2 }));
  expect(invoke).not.toHaveBeenCalledWith("refresh_repositories");

  await act(async () => repositoryChanged?.({ payload: { repositoryId: 1 } }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_status", { repositoryId: 1, includeIgnored: false }));
});

test("ignores an older repository summary response", async () => {
  let repositoryChanged: ((event: { payload: { repositoryId: number } }) => void) | undefined;
  const refreshResolvers: Array<(value: unknown) => void> = [];
  const repository = { id: 1, path: "/repo", name: "Repo", favorite: false, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "11111111", changedCount: 0, conflictCount: 0, ahead: 0, behind: 0 };
  vi.mocked(listen).mockImplementation(((event: string, handler: typeof repositoryChanged) => {
    if (event === "repository-changed") repositoryChanged = handler;
    return Promise.resolve(() => {});
  }) as never);
  vi.mocked(invoke).mockImplementation((command: string) => {
    if (command === "bootstrap") return Promise.resolve({ git: { supported: true, version: "2.50.1", path: "/usr/bin/git" }, settings: { selectedRepositoryId: 1, leftWidth: 240, rightWidth: 360, outputHeight: 190 }, repositories: [repository] });
    if (command === "get_status") return Promise.resolve({ id: 1, repositoryId: 1, headOid: "11111111", files: [] });
    if (command === "refresh_repository") return new Promise((resolve) => refreshResolvers.push(resolve));
    return Promise.resolve(undefined);
  });

  render(<App />);
  await screen.findByRole("option", { name: /Repo/ });
  await act(async () => {
    repositoryChanged?.({ payload: { repositoryId: 1 } });
    repositoryChanged?.({ payload: { repositoryId: 1 } });
  });
  await waitFor(() => expect(refreshResolvers).toHaveLength(2));
  await act(async () => refreshResolvers[1]({ ...repository, branch: "newer", changedCount: 2 }));
  await waitFor(() => expect(screen.getByRole("option", { name: /Repo/ })).toHaveTextContent("newer"));
  await act(async () => refreshResolvers[0]({ ...repository, branch: "older", changedCount: 1 }));
  expect(screen.getByRole("option", { name: /Repo/ })).toHaveTextContent("newer");
  expect(screen.getByRole("option", { name: /Repo/ })).not.toHaveTextContent("older");
});

test("ignores a stale status response after switching repositories", async () => {
  const staleResolvers: Array<(value: unknown) => void> = [];
  const repositories = [
    { id: 1, path: "/one", name: "One", favorite: false, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "11111111", changedCount: 1, conflictCount: 0, ahead: 0, behind: 0 },
    { id: 2, path: "/two", name: "Two", favorite: false, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "22222222", changedCount: 1, conflictCount: 0, ahead: 0, behind: 0 },
  ];
  vi.mocked(listen).mockImplementation(() => Promise.resolve(() => {}));
  vi.mocked(invoke).mockImplementation((command: string, args?: Parameters<typeof invoke>[1]) => {
    if (command === "bootstrap") return Promise.resolve({ git: { supported: true, version: "2.50.1", path: "/usr/bin/git" }, settings: { selectedRepositoryId: 1, leftWidth: 240, rightWidth: 360, outputHeight: 190 }, repositories });
    if (command === "get_status" && args && "repositoryId" in args && args.repositoryId === 1) return new Promise((resolve) => staleResolvers.push(resolve));
    if (command === "get_status") return Promise.resolve({ id: 2, repositoryId: 2, headOid: "22222222", files: [{ path: "new.ts", kind: "Modified", staged: false, unstaged: true, conflict: false, ignored: false }] });
    return Promise.resolve(undefined);
  });

  render(<App />);
  fireEvent.click(await screen.findByRole("option", { name: /Two/ }));
  expect(await screen.findByText("new.ts")).toBeInTheDocument();
  await act(async () => staleResolvers.forEach((resolve) => resolve({ id: 1, repositoryId: 1, headOid: "11111111", files: [{ path: "old.ts", kind: "Modified", staged: false, unstaged: true, conflict: false, ignored: false }] })));
  expect(screen.getByText("new.ts")).toBeInTheDocument();
  expect(screen.queryByText("old.ts")).not.toBeInTheDocument();
});

test("does not reload branch data for unrelated operation events", async () => {
  let operationListener: ((event: { payload: { operationId: number; repositoryId: number; kind: "started" | "finished"; message: string; outcome?: "succeeded" } }) => void) | undefined;
  vi.mocked(listen).mockImplementation(((event: string, handler: typeof operationListener) => {
    if (event === "operation-event") operationListener = handler;
    return Promise.resolve(() => {});
  }) as never);
  vi.mocked(invoke).mockImplementation((command: string) => {
    if (command === "bootstrap") return Promise.resolve({
      git: { supported: true, version: "2.50.1", path: "/usr/bin/git" }, settings: { selectedRepositoryId: 1, leftWidth: 240, rightWidth: 360, outputHeight: 190 },
      repositories: [{ id: 1, path: "/repo", name: "Repo", favorite: false, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: "12345678", changedCount: 0, conflictCount: 0, ahead: 0, behind: 0 }],
    });
    if (command === "get_status") return Promise.resolve({ id: 1, repositoryId: 1, headOid: "12345678", files: [] });
    if (command === "get_branches") return Promise.resolve([{ name: "main", oid: "12345678", current: true, remote: false }]);
    if (["get_tags", "get_remotes", "get_submodules"].includes(command)) return Promise.resolve([]);
    return Promise.resolve(undefined);
  });

  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Branches" }));
  await screen.findByText("Local branches");
  expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "get_branches")).toHaveLength(1);
  await act(async () => operationListener?.({ payload: { operationId: 3, repositoryId: 1, kind: "started", message: "Fetch" } }));
  await act(async () => operationListener?.({ payload: { operationId: 3, repositoryId: 1, kind: "finished", message: "Done", outcome: "succeeded" } }));
  expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "get_branches")).toHaveLength(1);
});

test("automatically loads history at the sentinel and keeps the DOM windowed", async () => {
  let observe: ((entries: Array<{ isIntersecting: boolean }>) => void) | undefined;
  class TestIntersectionObserver {
    constructor(callback: typeof observe) { observe = callback; }
    observe() { observe?.([{ isIntersecting: true }]); }
    disconnect() {}
  }
  vi.stubGlobal("IntersectionObserver", TestIntersectionObserver);
  const oid = (index: number) => index.toString(16).padStart(40, "0");
  const commits = Array.from({ length: 600 }, (_, index) => ({ oid: oid(index), parents: index === 0 ? [oid(599)] : [], author: "Ada", authoredAt: "2026-08-09T00:00:00Z", subject: `Commit ${index}`, refs: [], lane: { column: 0, parentColumns: index === 0 ? [0] : [] } }));
  vi.mocked(invoke).mockImplementation((command: string, args?: Parameters<typeof invoke>[1]) => {
    if (command === "bootstrap") return Promise.resolve({ git: { supported: true, version: "2.50.1", path: "/usr/bin/git" }, settings: { selectedRepositoryId: 1, leftWidth: 240, rightWidth: 360, outputHeight: 190 }, repositories: [{ id: 1, path: "/repo", name: "Repo", favorite: false, order: 0, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", headOid: commits[0].oid, changedCount: 0, conflictCount: 0, ahead: 0, behind: 0 }] });
    if (command === "get_status") return Promise.resolve({ id: 1, repositoryId: 1, files: [] });
    if (command === "get_history" && args && "cursor" in args && args.cursor) return Promise.resolve({ commits: commits.slice(1), nextCursor: null });
    if (command === "get_history") return Promise.resolve({ commits: commits.slice(0, 1), nextCursor: { offset: 1, activeLanes: [] } });
    return Promise.resolve(undefined);
  });
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "History" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_history", { repositoryId: 1, cursor: { offset: 1, activeLanes: [] }, limit: 100 }));
  await screen.findByText("600 commits loaded");
  expect(document.querySelectorAll(".graph-row").length).toBeLessThan(60);
  expect(document.querySelectorAll(".history-pane .object-action-row").length).toBeLessThan(60);
  const graph = document.querySelector<HTMLElement>(".graph-list")!;
  graph.scrollTop = 300 * 34;
  fireEvent.scroll(graph);
  await waitFor(() => expect(document.querySelectorAll(".graph-edge")).toHaveLength(1));
  vi.unstubAllGlobals();
});

test("groups repositories, moves one into favorites, and disables drag while searching", async () => {
  const repositories = [
    { id: 1, path: "/alpha", name: "Alpha", group: "Work", favorite: false, order: 0, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", changedCount: 0, conflictCount: 0, ahead: 0, behind: 0 },
    { id: 2, path: "/beta", name: "Beta", group: "Work", favorite: false, order: 1, kind: "workTree", capabilities: { canRead: true, canWriteWorkTree: true, canManageRefs: true, canManageRemotes: true }, branch: "main", changedCount: 0, conflictCount: 0, ahead: 0, behind: 0 },
  ];
  vi.mocked(invoke).mockImplementation((command: string) => {
    if (command === "bootstrap") return Promise.resolve({ git: { supported: true, version: "2.50.1", path: "/usr/bin/git" }, settings: { selectedRepositoryId: 1, leftWidth: 240, rightWidth: 360, outputHeight: 190 }, repositories });
    if (command === "get_status") return Promise.resolve({ id: 1, repositoryId: 1, files: [] });
    return Promise.resolve(undefined);
  });
  render(<App />);
  const search = await screen.findByRole("textbox", { name: "Search repositories" });
  fireEvent.change(search, { target: { value: "a" } });
  const filteredAlpha = await screen.findByRole("option", { name: /Alpha/ });
  expect(filteredAlpha).toHaveAttribute("draggable", "false");
  expect([...filteredAlpha.querySelectorAll<HTMLButtonElement>(".row-menu-popover button")].every((button) => button.disabled)).toBe(true);
  fireEvent.change(search, { target: { value: "" } });
  const alpha = await screen.findByRole("option", { name: /Alpha/ });
  const favorites = screen.getByRole("button", { name: /Favorites0/ }).closest("section")!;
  fireEvent.dragStart(alpha, { dataTransfer: { effectAllowed: "move", setData: vi.fn() } });
  fireEvent.drop(favorites, { dataTransfer: { getData: () => "1" } });
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("reorder_repositories", { placements: expect.arrayContaining([expect.objectContaining({ id: 1, favorite: true, group: "Work" })]) }));
  expect(screen.getByRole("button", { name: /Favorites1/ })).toBeInTheDocument();
});

test("exports the retained session log only after an explicit save choice", async () => {
  let operationListener: ((event: { payload: { operationId: number; repositoryId: number; kind: "stderr"; message: string } }) => void) | undefined;
  vi.mocked(listen).mockImplementation(((event: string, handler: typeof operationListener) => {
    if (event === "operation-event") operationListener = handler;
    return Promise.resolve(() => {});
  }) as never);
  vi.mocked(invoke).mockImplementation((command: string) => command === "bootstrap" ? Promise.resolve({ git: { supported: true, version: "2.50.1", path: "/usr/bin/git" }, settings: {}, repositories: [] }) : Promise.resolve(true));
  render(<App />);
  await act(async () => operationListener?.({ payload: { operationId: 1, repositoryId: 1, kind: "stderr", message: "https://user:token@example.com/repo" } }));
  await act(async () => operationListener?.({ payload: { operationId: 1, repositoryId: 1, kind: "stderr", message: "x".repeat(5 * 1024 * 1024) } }));
  fireEvent.click(await screen.findByRole("button", { name: "Export log" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("export_session_log", { fileName: expect.stringMatching(/^gitdock-session-.+\.log$/), lines: [expect.objectContaining({ kind: "stderr", message: "https://user:token@example.com/repo", timestamp: expect.any(String) })] }));
});
