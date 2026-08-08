import "@testing-library/jest-dom/vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import App from "./App";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn((command: string) => command === "bootstrap" ? Promise.resolve({ git: { supported: true, version: "2.50.1", path: "/usr/bin/git" }, settings: {}, repositories: [] }) : Promise.resolve([])) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: vi.fn(() => ({ onCloseRequested: vi.fn(() => Promise.resolve(() => {})), close: vi.fn() })) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(() => Promise.resolve(null)) }));

test("shows actionable first-run state", async () => {
  render(<App />);
  expect(await screen.findByText(/Put every working tree/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Add repository" })).toBeEnabled();
  fireEvent.click(screen.getByRole("button", { name: "Initialize" }));
});
