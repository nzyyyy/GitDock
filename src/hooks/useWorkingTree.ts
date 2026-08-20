import { useCallback, useRef, useState } from "react";
import { api, type CommitDetail, type ConflictDocument, type DiffFile, type FileChange, type WorkingTreeSnapshot } from "../api";
import { translate } from "../i18n";
import { errorMessage } from "../types";

async function loadFileDiffs(repositoryId: number, snapshotId: number, file: FileChange) {
  const sides: DiffFile[] = [];
  if (file.staged) sides.push(await api.getDiff(repositoryId, snapshotId, file.path, true));
  if (file.unstaged) sides.push(await api.getDiff(repositoryId, snapshotId, file.path, false));
  return sides;
}

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
  const [companionDiff, setCompanionDiff] = useState<DiffFile>();
  const [diffSnapshotId, setDiffSnapshotId] = useState<number>();
  const [conflict, setConflict] = useState<ConflictDocument & { snapshotId: number }>();
  const [selectedCommit, setSelectedCommit] = useState<string>();
  const [commitDetail, setCommitDetail] = useState<CommitDetail>();
  const [diffIsFile, setDiffIsFile] = useState(false);
  const statusRequest = useRef(0);
  const snapshotRef = useRef(snapshot);
  snapshotRef.current = snapshot;
  const diffRef = useRef(diff);
  diffRef.current = diff;

  const clearFileDiff = () => {
    setDiff(undefined); setCompanionDiff(undefined); setDiffSnapshotId(undefined); setDiffIsFile(false);
  };

  const showFileDiffs = (sides: DiffFile[], snapshotId: number) => {
    const usable = sides.filter((side) => side.patch || side.hunks.length || side.binary || side.tooLarge);
    const staged = usable.find((side) => side.staged);
    const unstaged = usable.find((side) => !side.staged);
    const primary = staged ?? unstaged;
    if (!primary) { clearFileDiff(); return; }
    setDiff(primary);
    setCompanionDiff(staged && unstaged ? unstaged : undefined);
    setDiffSnapshotId(snapshotId);
    setDiffIsFile(true);
    setConflict(undefined);
  };

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

  const openDiff = useCallback(async (file: FileChange, _staged?: boolean) => {
    if (!selectedId || !snapshot) return;
    const repositoryId = selectedId;
    const snapshotId = snapshot.id;
    try {
      if (file.conflict) {
        const document = await api.getConflictDocument(repositoryId, snapshotId, file.path);
        if (selectedIdRef.current === repositoryId) { setConflict({ ...document, snapshotId }); clearFileDiff(); }
      } else {
        const sides = await loadFileDiffs(repositoryId, snapshotId, file);
        if (selectedIdRef.current === repositoryId) showFileDiffs(sides, snapshotId);
      }
    }
    catch (error) { reportError(errorMessage(error)); }
  }, [selectedId, snapshot, reportError, selectedIdRef]);

  const closeDiff = useCallback(() => { clearFileDiff(); setConflict(undefined); setSelectedCommit(undefined); setCommitDetail(undefined); }, []);
  const closeCommitFile = useCallback(() => { clearFileDiff(); }, []);
  const loadIgnored = useCallback(() => refreshStatus(selectedId, !(snapshot?.files.some((file) => file.ignored) ?? false)), [selectedId, snapshot, refreshStatus]);

  const reloadOpenDiff = useCallback(async () => {
    const current = diffRef.current;
    if (!current?.hunks.length && !current?.binary && !current?.tooLarge) return;
    const repositoryId = selectedIdRef.current;
    if (!repositoryId) return;
    const includeIgnored = snapshotRef.current?.files.some((file) => file.ignored) ?? false;
    const nextSnapshot = await refreshStatus(repositoryId, includeIgnored);
    if (!nextSnapshot || selectedIdRef.current !== repositoryId) return;
    const file = nextSnapshot.files.find((item) => item.path === current.path);
    if (!file || file.conflict || (!file.staged && !file.unstaged)) { clearFileDiff(); return; }
    try {
      const sides = await loadFileDiffs(repositoryId, nextSnapshot.id, file);
      if (selectedIdRef.current !== repositoryId) return;
      showFileDiffs(sides, nextSnapshot.id);
    } catch (error) { reportError(errorMessage(error)); }
  }, [refreshStatus, reportError, selectedIdRef]);

  const openCommit = useCallback(async (oid: string) => {
    if (!selectedId) return;
    const repositoryId = selectedId;
    setSelectedCommit(oid);
    setConflict(undefined);
    clearFileDiff();
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
      if (historyRepositoryRef.current === repositoryId) {
        setCompanionDiff(undefined);
        setDiffSnapshotId(undefined);
        setDiff({ path, staged: false, binary: false, tooLarge: false, patch, hunks: [] });
        setDiffIsFile(false);
      }
    }
    catch (error) { reportError(errorMessage(error)); }
  }, [selectedId, selectedCommit, reportError, historyRepositoryRef]);

  const showBranchDiff = useCallback((value: string) => {
    setConflict(undefined);
    setCompanionDiff(undefined);
    setDiffSnapshotId(undefined);
    setDiffIsFile(false);
    setDiff({ path: translate(language, "branchComparison"), staged: false, binary: false, tooLarge: false, patch: value, hunks: [] });
  }, [language]);

  return {
    snapshot, setSnapshot, diff, companionDiff, diffSnapshotId, conflict, setConflict, selectedCommit, setSelectedCommit, commitDetail, diffIsFile,
    statusRequest, refreshStatus, reloadOpenDiff, openDiff, closeDiff, closeCommitFile, loadIgnored, openCommit, openCommitFile, showBranchDiff,
  };
}
