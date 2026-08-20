import { useCallback, useRef, useState } from "react";
import { api, type CommitDetail, type ConflictDocument, type DiffFile, type FileChange, type WorkingTreeSnapshot } from "../api";
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
  const [selectedCommit, setSelectedCommit] = useState<string>();
  const [commitDetail, setCommitDetail] = useState<CommitDetail>();
  const [diffIsFile, setDiffIsFile] = useState(false);
  const statusRequest = useRef(0);
  const snapshotRef = useRef(snapshot);
  snapshotRef.current = snapshot;
  const diffRef = useRef(diff);
  diffRef.current = diff;

  const refreshStatus = useCallback(async (repositoryId = selectedIdRef.current, includeIgnored = false) => {
    if (!repositoryId) return;
    const request = ++statusRequest.current;
    try {
      const value = await api.getStatus(repositoryId, includeIgnored);
      if (request !== statusRequest.current || repositoryId !== selectedIdRef.current) return;
      setSnapshot(value);
      return value;
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
        const document = await api.getConflictDocument(repositoryId, snapshotId, file.path);
        if (selectedIdRef.current === repositoryId) { setConflict({ ...document, snapshotId }); setDiff(undefined); setDiffIsFile(false); }
      } else {
        const nextDiff = await api.getDiff(repositoryId, snapshotId, file.path, staged);
        if (selectedIdRef.current === repositoryId) { setDiff(nextDiff); setConflict(undefined); setDiffIsFile(true); }
      }
    }
    catch (error) { reportError(errorMessage(error)); }
  }, [selectedId, snapshot, reportError, selectedIdRef]);

  const closeDiff = useCallback(() => { setDiff(undefined); setConflict(undefined); setDiffIsFile(false); setSelectedCommit(undefined); setCommitDetail(undefined); }, []);
  const closeCommitFile = useCallback(() => { setDiff(undefined); setDiffIsFile(false); }, []);
  const loadIgnored = useCallback(() => refreshStatus(selectedId, !(snapshot?.files.some((file) => file.ignored) ?? false)), [selectedId, snapshot, refreshStatus]);

  const reloadOpenDiff = useCallback(async () => {
    const current = diffRef.current;
    if (!current?.hunks.length) return;
    const repositoryId = selectedIdRef.current;
    if (!repositoryId) return;
    const includeIgnored = snapshotRef.current?.files.some((file) => file.ignored) ?? false;
    const nextSnapshot = await refreshStatus(repositoryId, includeIgnored);
    if (!nextSnapshot || selectedIdRef.current !== repositoryId) return;
    const file = nextSnapshot.files.find((item) => item.path === current.path);
    const stillOpen = file && !file.conflict && (current.staged ? file.staged : file.unstaged);
    if (!stillOpen) { setDiff(undefined); setDiffIsFile(false); return; }
    try {
      const nextDiff = await api.getDiff(repositoryId, nextSnapshot.id, current.path, current.staged);
      if (selectedIdRef.current !== repositoryId) return;
      if (!nextDiff.patch && !nextDiff.hunks.length) { setDiff(undefined); setDiffIsFile(false); return; }
      setDiff(nextDiff);
    } catch (error) { reportError(errorMessage(error)); }
  }, [refreshStatus, reportError, selectedIdRef]);

  const openCommit = useCallback(async (oid: string) => {
    if (!selectedId) return;
    const repositoryId = selectedId;
    setSelectedCommit(oid);
    setConflict(undefined);
    setDiff(undefined);
    setDiffIsFile(false);
    setCommitDetail(undefined);
    try {
      const detail = await api.getCommitDetail(repositoryId, oid);
      if (historyRepositoryRef.current === repositoryId) setCommitDetail(detail);
    }
    catch (error) { reportError(errorMessage(error)); }
  }, [selectedId, reportError, historyRepositoryRef]);

  const openCommitFile = useCallback(async (path: string) => {
    if (!selectedId || !selectedCommit) return;
    const repositoryId = selectedId;
    const oid = selectedCommit;
    try {
      const patch = await api.getCommitFileDiff(repositoryId, oid, path);
      if (historyRepositoryRef.current === repositoryId) setDiff({ path, staged: false, binary: false, tooLarge: false, patch, hunks: [] });
    }
    catch (error) { reportError(errorMessage(error)); }
  }, [selectedId, selectedCommit, reportError, historyRepositoryRef]);

  const showBranchDiff = useCallback((value: string) => {
    setConflict(undefined);
    setDiffIsFile(false);
    setDiff({ path: translate(language, "branchComparison"), staged: false, binary: false, tooLarge: false, patch: value, hunks: [] });
  }, [language]);

  return {
    snapshot, setSnapshot, diff, conflict, setConflict, selectedCommit, setSelectedCommit, commitDetail, diffIsFile,
    statusRequest, refreshStatus, reloadOpenDiff, openDiff, closeDiff, closeCommitFile, loadIgnored, openCommit, openCommitFile, showBranchDiff,
  };
}
