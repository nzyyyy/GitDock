import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, type CommitInfo, type HistoryCursor } from "../api";
import { errorMessage, type Tab } from "../types";

export function useHistory({
  reportError, selectedIdRef, selectedId, tab,
}: {
  reportError: (message: string) => void;
  selectedIdRef: React.MutableRefObject<number | undefined>;
  selectedId?: number;
  tab: Tab;
}) {
  const [commits, setCommits] = useState<CommitInfo[]>([]);
  const [nextHistoryCursor, setNextHistoryCursor] = useState<HistoryCursor>();
  const [selectedCommit, setSelectedCommit] = useState<string>();
  const [historyLoading, setHistoryLoading] = useState(false);
  const historyRepository = useRef<number | undefined>(undefined);
  const historyRequest = useRef(0);

  const refreshHistory = useCallback(async (repositoryId: number) => {
    const request = ++historyRequest.current;
    historyRepository.current = repositoryId;
    setCommits([]); setNextHistoryCursor(undefined); setSelectedCommit(undefined); setHistoryLoading(true);
    try {
      const page = await api.history(repositoryId);
      if (request !== historyRequest.current || repositoryId !== selectedIdRef.current) return;
      setCommits(page.commits); setNextHistoryCursor(page.nextCursor ?? undefined);
    } catch (error) {
      if (request !== historyRequest.current || repositoryId !== selectedIdRef.current) return;
      historyRepository.current = undefined; reportError(errorMessage(error));
    } finally {
      if (request === historyRequest.current && repositoryId === selectedIdRef.current) setHistoryLoading(false);
    }
  }, [reportError, selectedIdRef]);

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
    if (tab !== "history") return;
    const unlisten = listen<{ repositoryId: number }>("repository-changed", ({ payload }) => {
      if (selectedIdRef.current === payload.repositoryId) void refreshHistory(payload.repositoryId);
    });
    return () => { unlisten.then((unlistenFn) => unlistenFn()); };
  }, [tab, refreshHistory, selectedIdRef]);

  const loadMoreHistory = useCallback(async () => {
    if (!selectedId || historyRepository.current !== selectedId || nextHistoryCursor === undefined || historyLoading) return;
    const repositoryId = selectedId;
    const request = historyRequest.current;
    setHistoryLoading(true);
    try {
      const page = await api.history(repositoryId, nextHistoryCursor);
      if (request !== historyRequest.current || historyRepository.current !== repositoryId) return;
      setCommits((current) => [...new Map([...current, ...page.commits].map((commit) => [commit.oid, commit])).values()]);
      setNextHistoryCursor(page.nextCursor ?? undefined);
    } catch (error) {
      if (request !== historyRequest.current || historyRepository.current !== repositoryId) return;
      reportError(errorMessage(error));
    } finally {
      if (request === historyRequest.current && historyRepository.current === repositoryId) setHistoryLoading(false);
    }
  }, [selectedId, nextHistoryCursor, historyLoading, reportError]);

  return {
    commits, nextHistoryCursor, selectedCommit, setSelectedCommit, historyLoading,
    hasMore: historyRepository.current === selectedId && nextHistoryCursor !== undefined,
    historyRepositoryRef: historyRepository,
    refreshHistory, loadMoreHistory,
  };
}
