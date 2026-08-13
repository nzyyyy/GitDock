import { useCallback, useState } from "react";
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

  const close = useCallback(() => {
    setView(undefined); setPath(undefined); setEntries([]);
    setSelectedOid(undefined); setDiff(undefined); setBlameFile(undefined);
  }, []);

  const openFileHistory = useCallback(async (target: string) => {
    const repositoryId = selectedId;
    if (!repositoryId) return;
    setView("history"); setPath(target); setEntries([]); setSelectedOid(undefined); setDiff(undefined);
    try {
      const list = await api.fileHistory(repositoryId, target);
      if (selectedIdRef.current === repositoryId) setEntries(list);
    } catch (error) { reportError(errorMessage(error)); }
  }, [selectedId, selectedIdRef, reportError]);

  const selectHistoryOid = useCallback(async (oid: string) => {
    const repositoryId = selectedId;
    const currentPath = path;
    if (!repositoryId || !currentPath) return;
    setSelectedOid(oid); setDiff(undefined);
    try {
      const patch = await api.commitFileDiff(repositoryId, oid, currentPath);
      if (selectedIdRef.current === repositoryId) setDiff(patch);
    } catch (error) { reportError(errorMessage(error)); }
  }, [selectedId, selectedIdRef, path, reportError]);

  const openBlame = useCallback(async (target: string) => {
    const repositoryId = selectedId;
    if (!repositoryId) return;
    setView("blame"); setPath(target); setBlameFile(undefined);
    try {
      const file = await api.blame(repositoryId, target);
      if (selectedIdRef.current === repositoryId) setBlameFile(file);
    } catch (error) { reportError(errorMessage(error)); }
  }, [selectedId, selectedIdRef, reportError]);

  return { view, path, entries, selectedOid, diff, blameFile, openFileHistory, openBlame, selectHistoryOid, close };
}
