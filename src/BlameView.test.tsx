import "@testing-library/jest-dom/vitest";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import type { BlameFile } from "./api";
import { BlameView } from "./components/BlameView";
import { I18nProvider } from "./i18n";

afterEach(cleanup);

const blame: BlameFile = {
  path: "src/file.txt",
  content: ["a", "c"],
  hunks: [
    { oid: "a".repeat(40), author: "Alice", authorTime: 0, startLine: 1, lineCount: 1 },
    { oid: "b".repeat(40), author: "Bob", authorTime: 0, startLine: 2, lineCount: 1 },
  ],
};

test("renders content lines with hunk authors and line numbers", () => {
  render(<I18nProvider language="en"><BlameView blame={blame} onBack={vi.fn()} /></I18nProvider>);
  expect(screen.getByText("a")).toBeInTheDocument();
  expect(screen.getByText("c")).toBeInTheDocument();
  expect(screen.getByText(/Alice/)).toBeInTheDocument();
  expect(screen.getByText(/Bob/)).toBeInTheDocument();
  expect(document.querySelector(".blame-author")).toHaveTextContent(new Intl.DateTimeFormat("en").format(new Date(0)));
  expect(screen.getAllByText("1").length).toBeGreaterThan(0);
});


test("distinguishes loading, failure and empty blame without showing stale content", () => {
  const view = (loading: boolean, error?: string) => <I18nProvider language="en"><BlameView path="next.ts" blame={blame} loading={loading} error={error} onBack={vi.fn()} /></I18nProvider>;
  const { rerender } = render(view(true));
  expect(screen.getByRole("status")).toHaveTextContent("Loading…");
  expect(screen.queryByText("a")).not.toBeInTheDocument();
  rerender(view(false, "cannot read file"));
  expect(screen.getByRole("alert")).toHaveTextContent("cannot read file");
  expect(screen.queryByText("a")).not.toBeInTheDocument();
  rerender(<I18nProvider language="en"><BlameView blame={{ ...blame, content: [], hunks: [] }} onBack={vi.fn()} /></I18nProvider>);
  expect(screen.getByRole("status")).toHaveTextContent("Empty file");
});
