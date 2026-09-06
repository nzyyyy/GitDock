import "@testing-library/jest-dom/vitest";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { FileHistoryView } from "./components/FileHistoryView";
import { I18nProvider } from "./i18n";

afterEach(cleanup);

test("file history distinguishes loading, failure and an empty result", () => {
  const view = (loading: boolean, error?: string) => <I18nProvider language="en"><FileHistoryView path="file.ts" entries={[]} loading={loading} error={error} onBack={vi.fn()} onSelect={vi.fn()} /></I18nProvider>;
  const { rerender } = render(view(true));
  expect(screen.getByRole("status")).toHaveTextContent("Loading…");
  expect(screen.queryByText("No history for this file")).not.toBeInTheDocument();
  rerender(view(false, "cannot read history"));
  expect(screen.getByRole("alert")).toHaveTextContent("cannot read history");
  expect(screen.queryByText("No history for this file")).not.toBeInTheDocument();
  rerender(view(false));
  expect(screen.getByText("No history for this file")).toBeInTheDocument();
});
