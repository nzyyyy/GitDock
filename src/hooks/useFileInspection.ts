import { useCallback, useEffect, useRef, useState } from "react";
import { api, type BlameFile, type FileHistoryEntry } from "../api";
import { errorMessage } from "../types";

export function useFileInspection({ reportError, selectedId, selectedIdRef }: {
  reportError: (message: string) => void;
  selectedId?: number;
  selectedIdRef: React.MutableRefObject<number | undefined>;
}) {
  const [view, setView] = useState<"history" | "blame" | undefined>();
  const [path, setPath] = useState<string>();
  const [entries, setEntries] = useState<FileHistoryEntry[]>([]);
  const [selectedOid, setSelectedOid] = useState<string>();
  const [diff, setDiff] = useState<string>();
  const [blameFile, setBlameFile] = useState<BlameFile>();

  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string>();
  const requestId = useRef(0);

  const close = useCallback(() => {
    requestId.current += 1; setLoading(false); setError(undefined);
    setView(undefined); setPath(undefined); setEntries([]);
    setSelectedOid(undefined); setDiff(undefined); setBlameFile(undefined);
  }, []);

  useEffect(() => { close(); return () => { requestId.current += 1; }; }, [selectedId, close]);

  const openFileHistory = useCallback(async (target: string) => {
    const repositoryId = selectedId;
    if (!repositoryId) return;
    close();
    const request = ++requestId.current;
    setLoading(true);
    setView("history"); setPath(target);
    try {
      const list = await api.getFileHistory(repositoryId, target);
      if (request === requestId.current && selectedIdRef.current === repositoryId) setEntries(list);
    } catch (cause) {
      if (request === requestId.current && selectedIdRef.current === repositoryId) {
        const message = errorMessage(cause); setError(message); reportError(message);
      }
    } finally {
      if (request === requestId.current && selectedIdRef.current === repositoryId) setLoading(false);
    }
  }, [selectedId, selectedIdRef, reportError, close]);

  const selectHistoryOid = useCallback(async (oid: string) => {
    const repositoryId = selectedId;
    const currentPath = path;
    if (!repositoryId || !currentPath) return;
    const request = ++requestId.current;
    setLoading(true); setError(undefined);
    setSelectedOid(oid); setDiff(undefined);
    try {
      const patch = await api.getCommitFileDiff(repositoryId, oid, currentPath);
      if (request === requestId.current && selectedIdRef.current === repositoryId) setDiff(patch);
    } catch (cause) {
      if (request === requestId.current && selectedIdRef.current === repositoryId) {
        const message = errorMessage(cause); setError(message); reportError(message);
      }
    } finally {
      if (request === requestId.current && selectedIdRef.current === repositoryId) setLoading(false);
    }
  }, [selectedId, selectedIdRef, path, reportError]);

  const openBlame = useCallback(async (target: string) => {
    const repositoryId = selectedId;
    if (!repositoryId) return;
    close();
    const request = ++requestId.current;
    setLoading(true);
    setView("blame"); setPath(target);
    try {
      const file = await api.getBlame(repositoryId, target);
      if (request === requestId.current && selectedIdRef.current === repositoryId) setBlameFile(file);
    } catch (cause) {
      if (request === requestId.current && selectedIdRef.current === repositoryId) {
        const message = errorMessage(cause); setError(message); reportError(message);
      }
    } finally {
      if (request === requestId.current && selectedIdRef.current === repositoryId) setLoading(false);
    }
  }, [selectedId, selectedIdRef, reportError, close]);

  return { loading, error, view, path, entries, selectedOid, diff, blameFile, openFileHistory, openBlame, selectHistoryOid, close };
}
