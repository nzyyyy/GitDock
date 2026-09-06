import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { api, type BlameFile, type FileHistoryEntry } from "../api";
import { useFileInspection } from "./useFileInspection";

vi.mock("../api", () => ({ api: { getFileHistory: vi.fn(), getCommitFileDiff: vi.fn(), getBlame: vi.fn() } }));
afterEach(() => { cleanup(); vi.resetAllMocks(); });

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function setup() {
  const reportError = vi.fn();
  const selectedIdRef = { current: 1 as number | undefined };
  const hook = renderHook(({ id }) => {
    selectedIdRef.current = id;
    return useFileInspection({ selectedId: id, selectedIdRef, reportError });
  }, { initialProps: { id: 1 } });
  return { ...hook, reportError };
}

const blame: BlameFile = { path: "new.ts", content: ["current"], hunks: [] };

test.each([false, true])("same-repository file history ignores stale results and errors (reject=%s)", async (reject) => {
  const old = deferred<FileHistoryEntry[]>();
  const next = deferred<FileHistoryEntry[]>();
  vi.mocked(api.getFileHistory).mockReturnValueOnce(old.promise).mockReturnValueOnce(next.promise);
  const { result, reportError } = setup();
  act(() => { void result.current.openFileHistory("old.ts"); });
  act(() => { void result.current.openFileHistory("new.ts"); });
  await act(async () => { if (reject) old.reject(new Error("obsolete")); else old.resolve([{ oid: "old" } as FileHistoryEntry]); });
  expect(result.current.path).toBe("new.ts");
  expect(result.current.entries).toEqual([]);
  expect(result.current.loading).toBe(true);
  expect(reportError).not.toHaveBeenCalled();
  await act(async () => { next.resolve([]); });
  expect(result.current.loading).toBe(false);
  expect(result.current.error).toBeUndefined();
});

test("rapid history commit selection keeps only the latest patch", async () => {
  vi.mocked(api.getFileHistory).mockResolvedValue([]);
  const old = deferred<string>();
  const next = deferred<string>();
  vi.mocked(api.getCommitFileDiff).mockReturnValueOnce(old.promise).mockReturnValueOnce(next.promise);
  const { result } = setup();
  await act(async () => { await result.current.openFileHistory("file.ts"); });
  act(() => { void result.current.selectHistoryOid("old"); });
  act(() => { void result.current.selectHistoryOid("new"); });
  await act(async () => { next.resolve("new patch"); });
  await act(async () => { old.resolve("old patch"); });
  expect(result.current.selectedOid).toBe("new");
  expect(result.current.diff).toBe("new patch");
  expect(result.current.loading).toBe(false);
});

test.each(["history", "blame"] as const)("switching away from %s ignores late failures", async (from) => {
  const old = deferred<never>();
  vi.mocked(api.getFileHistory).mockResolvedValue([]);
  vi.mocked(api.getBlame).mockResolvedValue(blame);
  const { result, reportError } = setup();
  if (from === "history") {
    vi.mocked(api.getFileHistory).mockReturnValueOnce(old.promise);
    act(() => { void result.current.openFileHistory("old.ts"); });
    await act(async () => { await result.current.openBlame("new.ts"); });
  } else {
    vi.mocked(api.getBlame).mockReturnValueOnce(old.promise);
    act(() => { void result.current.openBlame("old.ts"); });
    await act(async () => { await result.current.openFileHistory("new.ts"); });
  }
  await act(async () => { old.reject(new Error("obsolete")); });
  expect(result.current.path).toBe("new.ts");
  expect(result.current.view).toBe(from === "history" ? "blame" : "history");
  expect(result.current.error).toBeUndefined();
  expect(reportError).not.toHaveBeenCalled();
});

test("rapid blame selection clears old content and rejects out-of-order results", async () => {
  const old = deferred<BlameFile>();
  vi.mocked(api.getBlame).mockResolvedValueOnce(blame).mockReturnValueOnce(old.promise).mockResolvedValueOnce({ ...blame, path: "latest.ts" });
  const { result } = setup();
  await act(async () => { await result.current.openBlame("new.ts"); });
  act(() => { void result.current.openBlame("old.ts"); });
  expect(result.current.blameFile).toBeUndefined();
  expect(result.current.loading).toBe(true);
  await act(async () => { await result.current.openBlame("latest.ts"); });
  await act(async () => { old.resolve({ ...blame, path: "old.ts" }); });
  expect(result.current.blameFile?.path).toBe("latest.ts");
});

test.each(["close", "repository"] as const)("%s invalidates pending history, diff and blame requests", async (exit) => {
  const history = deferred<FileHistoryEntry[]>();
  const patch = deferred<string>();
  const pendingBlame = deferred<BlameFile>();
  vi.mocked(api.getFileHistory).mockReturnValue(history.promise);
  vi.mocked(api.getCommitFileDiff).mockReturnValue(patch.promise);
  vi.mocked(api.getBlame).mockReturnValue(pendingBlame.promise);
  const { result, rerender, reportError } = setup();
  act(() => { void result.current.openFileHistory("file.ts"); });
  act(() => { void result.current.selectHistoryOid("old"); });
  act(() => { void result.current.openBlame("file.ts"); });
  if (exit === "close") act(() => { result.current.close(); });
  else { rerender({ id: 2 }); rerender({ id: 1 }); }
  await act(async () => { history.resolve([]); patch.reject(new Error("obsolete")); pendingBlame.resolve(blame); });
  expect(result.current.view).toBeUndefined();
  expect(result.current.path).toBeUndefined();
  expect(result.current.diff).toBeUndefined();
  expect(result.current.blameFile).toBeUndefined();
  expect(result.current.loading).toBe(false);
  expect(reportError).not.toHaveBeenCalled();
});

test("current failure stops loading and is cleared by the next request", async () => {
  vi.mocked(api.getBlame).mockRejectedValueOnce(new Error("cannot read file")).mockResolvedValueOnce(blame);
  const { result, reportError } = setup();
  await act(async () => { await result.current.openBlame("bad.ts"); });
  expect(result.current.error).toBe("cannot read file");
  expect(result.current.loading).toBe(false);
  expect(reportError).toHaveBeenCalledWith("cannot read file");
  await act(async () => { await result.current.openBlame("new.ts"); });
  expect(result.current.error).toBeUndefined();
});
