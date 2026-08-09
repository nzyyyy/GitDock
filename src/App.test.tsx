import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
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
afterEach(cleanup);

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
  const menu = document.querySelector<HTMLDivElement>(".row-menu-popover")!;
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
