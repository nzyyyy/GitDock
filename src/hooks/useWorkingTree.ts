import { useCallback, useRef, useState } from "react";
import { api, type ConflictDocument, type DiffFile, type FileChange, type WorkingTreeSnapshot } from "../api";
import type { DiffMode } from "../DiffView";
import { translate } from "../i18n";
import { errorMessage } from "../types";

export function useWorkingTree({
  reportError, selectedIdRef, historyRepositoryRef, selectedId, language,
}: {
  reportError: (message: string) => void;
  selectedIdRef: React.MutableRefObject<number | undefined>;
  historyRepositoryRef: React.MutableRefObject<number | undefined>;
  selectedId?: number;
  language: "en" | "zh-CN";
}) {
  const [snapshot, setSnapshot] = useState<WorkingTreeSnapshot>();
  const [diff, setDiff] = useState<DiffFile>();
  const [conflict, setConflict] = useState<ConflictDocument & { snapshotId: number }>();
  const [diffMode, setDiffMode] = useState<DiffMode>("unified");
  const [selectedCommit, setSelectedCommit] = useState<string>();
  const [diffIsFile, setDiffIsFile] = useState(false);
  const statusRequest = useRef(0);

  const refreshStatus = useCallback(async (repositoryId = selectedIdRef.current, includeIgnored = false) => {
    if (!repositoryId) return;
    const request = ++statusRequest.current;
    try {
      const value = await api.status(repositoryId, includeIgnored);
      if (request === statusRequest.current && repositoryId === selectedIdRef.current) setSnapshot(value);
    } catch (error) {
      if (request !== statusRequest.current || repositoryId !== selectedIdRef.current) return;
      setSnapshot(undefined); reportError(errorMessage(error));
    }
  }, [reportError, selectedIdRef]);

  const openDiff = useCallback(async (file: FileChange, staged: boolean) => {
    if (!selectedId || !snapshot) return;
    const repositoryId = selectedId;
    const snapshotId = snapshot.id;
    try {
      if (file.conflict) {
        const document = await api.conflictDocument(repositoryId, snapshotId, file.path);
        if (selectedIdRef.current === repositoryId) { setConflict({ ...document, snapshotId }); setDiff(undefined); setDiffIsFile(false); }
      } else {
        const nextDiff = await api.diff(repositoryId, snapshotId, file.path, staged);
        if (selectedIdRef.current === repositoryId) { setDiff(nextDiff); setConflict(undefined); setDiffIsFile(true); }
      }
    }
    catch (error) { reportError(errorMessage(error)); }
  }, [selectedId, snapshot, reportError, selectedIdRef]);

  const closeDiff = useCallback(() => { setDiff(undefined); setConflict(undefined); setDiffIsFile(false); setSelectedCommit(undefined); }, []);
  const loadIgnored = useCallback(() => refreshStatus(selectedId, true), [selectedId, refreshStatus]);

  const openCommit = useCallback(async (oid: string) => {
    if (!selectedId) return;
    const repositoryId = selectedId;
    setSelectedCommit(oid);
    setConflict(undefined);
    setDiffIsFile(false);
    try {
      const patch = await api.commitDiff(repositoryId, oid);
      if (historyRepositoryRef.current === repositoryId) setDiff({ path: translate(language, "commitDiff"), staged: false, binary: false, tooLarge: false, patch, hunks: [] });
    }
    catch (error) { reportError(errorMessage(error)); }
  }, [selectedId, language, reportError, historyRepositoryRef]);

  const showBranchDiff = useCallback((value: string) => {
    setConflict(undefined);
    setDiffIsFile(false);
    setDiff({ path: translate(language, "branchComparison"), staged: false, binary: false, tooLarge: false, patch: value, hunks: [] });
  }, [language]);

  return {
    snapshot, setSnapshot, diff, conflict, setConflict, diffMode, setDiffMode, selectedCommit, setSelectedCommit, diffIsFile,
    statusRequest, refreshStatus, openDiff, closeDiff, loadIgnored, openCommit, showBranchDiff,
  };
}
