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
    if (command === "get_branches") return Promise.resolve([{ name: "main", oid: "12345678", current: true, remote: false }]);
    if (["get_tags", "get_remotes", "get_submodules"].includes(command)) return Promise.resolve([]);
    if (command === "preview_operation") return Promise.resolve({ title: "Create branch", summary: "", risk: "normal", affectedPaths: [], affectedRefs: [], recoverable: true, requiresConfirmation: false });
    if (command === "start_operation") return Promise.resolve({ operationId: 1, accepted: true });
    return Promise.resolve(undefined);
  });

  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Branches" }));
  const newBranch = await screen.findByRole("button", { name: "New branch" });
  fireEvent.click(newBranch);
  const input = screen.getByRole("textbox", { name: "New branch name" });
  expect(input).toHaveFocus();
  expect(screen.getByRole("button", { name: "Create" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "More actions" })).toHaveAttribute("aria-haspopup", "menu");
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
