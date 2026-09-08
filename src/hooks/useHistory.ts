import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, type BranchInfo, type CommitInfo, type HistoryCursor } from "../api";
import { errorMessage, type Tab } from "../types";

export function useHistory({
  reportError, selectedIdRef, selectedId, tab, onReset,
}: {
  reportError: (message: string) => void;
  selectedIdRef: React.MutableRefObject<number | undefined>;
  selectedId?: number;
  tab: Tab;
  onReset: () => void;
}) {
  const [commits, setCommits] = useState<CommitInfo[]>([]);
  const [nextHistoryCursor, setNextHistoryCursor] = useState<HistoryCursor>();
  const [branches, setBranches] = useState<BranchInfo[]>([]);
  const [branchRef, setBranchRef] = useState<string | null>(null);
  const scope = useRef<{ repositoryId?: number; branchRef: string | null }>({ branchRef: null });
  const [historyLoading, setHistoryLoading] = useState(false);
  const historyRepository = useRef<number | undefined>(undefined);
  const historyRequest = useRef(0);

  const refreshHistory = useCallback(async (repositoryId: number, requestedBranch = scope.current.repositoryId === repositoryId ? scope.current.branchRef : null) => {
    scope.current = { repositoryId, branchRef: requestedBranch };
    setBranchRef(requestedBranch);
    const request = ++historyRequest.current;
    historyRepository.current = repositoryId;
    setCommits([]); setNextHistoryCursor(undefined); onReset(); setHistoryLoading(true);
    try {
      const nextBranches = await api.getBranches(repositoryId);
      if (request !== historyRequest.current || repositoryId !== selectedIdRef.current) return;
      setBranches(nextBranches);
      const reference = nextBranches.some((branch) => `refs/${branch.remote ? "remotes" : "heads"}/${branch.name}` === requestedBranch) ? requestedBranch : null;
      scope.current.branchRef = reference;
      setBranchRef(reference);
      const page = await api.getHistory(repositoryId, null, 100, reference);
      if (request !== historyRequest.current || repositoryId !== selectedIdRef.current) return;
      setCommits(page.commits); setNextHistoryCursor(page.nextCursor ?? undefined);
    } catch (error) {
      if (request !== historyRequest.current || repositoryId !== selectedIdRef.current) return;
      historyRepository.current = undefined; reportError(errorMessage(error));
    } finally {
      if (request === historyRequest.current && repositoryId === selectedIdRef.current) setHistoryLoading(false);
    }
  }, [reportError, selectedIdRef, onReset]);

  useEffect(() => {
    historyRequest.current += 1;
    historyRepository.current = undefined;
    scope.current = { repositoryId: selectedId, branchRef: null };
    setBranchRef(null); setBranches([]); setCommits([]); setNextHistoryCursor(undefined); setHistoryLoading(false);
  }, [selectedId]);

  useEffect(() => {
    if (!selectedId || tab !== "history" || historyRepository.current === selectedId) return;
    let settled = false;
    void refreshHistory(selectedId).finally(() => { settled = true; });
    return () => {
      if (!settled && historyRepository.current === selectedId) {
        historyRequest.current += 1; historyRepository.current = undefined; setHistoryLoading(false);
      }
    };
  }, [selectedId, tab, refreshHistory]);

  useEffect(() => {
    const unlisten = listen<{ repositoryId: number }>("repository-changed", ({ payload }) => {
      if (selectedIdRef.current !== payload.repositoryId) return;
      if (tab === "history") void refreshHistory(payload.repositoryId);
      else { historyRequest.current += 1; historyRepository.current = undefined; setHistoryLoading(false); }
    });
    return () => { unlisten.then((unlistenFn) => unlistenFn()); };
  }, [tab, refreshHistory, selectedIdRef]);

  const loadMoreHistory = useCallback(async () => {
    if (!selectedId || historyRepository.current !== selectedId || nextHistoryCursor === undefined || historyLoading) return;
    const repositoryId = selectedId;
    const request = historyRequest.current;
    setHistoryLoading(true);
    try {
      const page = await api.getHistory(repositoryId, nextHistoryCursor, 100, scope.current.branchRef);
      if (request !== historyRequest.current || historyRepository.current !== repositoryId || selectedIdRef.current !== repositoryId) return;
      setCommits((current) => [...new Map([...current, ...page.commits].map((commit) => [commit.oid, commit])).values()]);
      setNextHistoryCursor(page.nextCursor ?? undefined);
    } catch (error) {
      if (request !== historyRequest.current || historyRepository.current !== repositoryId || selectedIdRef.current !== repositoryId) return;
      reportError(errorMessage(error));
    } finally {
      if (request === historyRequest.current && historyRepository.current === repositoryId && selectedIdRef.current === repositoryId) setHistoryLoading(false);
    }
  }, [selectedId, nextHistoryCursor, historyLoading, reportError, selectedIdRef]);

  return {
    commits, nextHistoryCursor, historyLoading, branches, branchRef,
    selectBranch: (reference: string | null) => { if (selectedId) void refreshHistory(selectedId, reference); },
    hasMore: historyRepository.current === selectedId && nextHistoryCursor !== undefined,
    historyRepositoryRef: historyRepository,
    refreshHistory, loadMoreHistory,
  };
}
