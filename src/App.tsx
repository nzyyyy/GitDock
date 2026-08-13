import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import { api, type BranchInfo, type CommitInfo, type ConflictDocument, type ConflictResolution, type DiffFile, type FileChange, type GitInfo, type HistoryCursor, type Language, type OperationEvent, type OperationPreview, type OperationRequest, type RemoteInfo, type RepositorySummary, type SessionLogLine, type StashInfo, type SubmoduleInfo, type TagInfo, type WorkingTreeSnapshot } from "./api";
import { ConflictEditor } from "./ConflictEditor";
import { DiffView, type DiffMode } from "./DiffView";
import { I18nProvider, translate, useI18n } from "./i18n";

type Tab = "changes" | "history" | "branches" | "stashes";
type LogLine = SessionLogLine & { id: number; bytes: number };
type LogBuffer = { entries: Array<LogLine | undefined>; start: number; length: number; bytes: number };
type RepositoryGroup = { key: string; label: string; repositories: RepositorySummary[] };
type OperationOutcome = NonNullable<OperationEvent["outcome"]>;
type OperationToast = { id: number; title: string; message: string; outcome: OperationOutcome };
type OperationFinished = (outcome: OperationOutcome) => void;
type RunOperation = (request: OperationRequest, onFinished?: OperationFinished) => void | Promise<void>;
type Pending = { repositoryId: number; request: OperationRequest; preview: OperationPreview; onFinished?: OperationFinished };
type DialogValue = string | boolean;
type DialogField = { name: string; label: string; value?: DialogValue; required?: boolean; type?: "text" | "checkbox" };
type DialogSpec = { title: string; message?: string; submitLabel?: string; danger?: boolean; fields?: DialogField[]; onSubmit: (values: Record<string, DialogValue>) => void | Promise<void> };
type CommandItem = { id: string; label: string; search: string; action: () => void };

const shortOid = (oid?: string) => oid?.slice(0, 8) ?? "—";
const errorMessage = (error: unknown) => error instanceof Error ? error.message : String(error);
const FAVORITES_GROUP = "\0favorites";
const UNGROUPED_GROUP = "\0ungrouped";
const LOG_LIMIT = 10_000;
const LOG_BYTE_LIMIT = 5 * 1024 * 1024;
const GRAPH_EDGE_BUCKET_ROWS = 256;
const textEncoder = new TextEncoder();

const newLogBuffer = (): LogBuffer => ({ entries: [], start: 0, length: 0, bytes: 0 });

const appendLog = (buffer: LogBuffer, line: Omit<LogLine, "bytes">) => {
  const bytes = textEncoder.encode(`${line.timestamp} ${line.kind} ${line.message}\n`).byteLength;
  if (bytes > LOG_BYTE_LIMIT) return false;
  while (buffer.length && (buffer.length >= LOG_LIMIT || buffer.bytes + bytes > LOG_BYTE_LIMIT)) {
    buffer.bytes -= buffer.entries[buffer.start]!.bytes;
    buffer.entries[buffer.start] = undefined;
    buffer.start = (buffer.start + 1) % LOG_LIMIT;
    buffer.length -= 1;
  }
  const index = (buffer.start + buffer.length) % LOG_LIMIT;
  buffer.entries[index] = { ...line, bytes };
  buffer.length += 1;
  buffer.bytes += bytes;
  return true;
};

const readLogs = (buffer: LogBuffer) => Array.from({ length: buffer.length }, (_, index) => buffer.entries[(buffer.start + index) % LOG_LIMIT]!);
const lastLog = (buffer: LogBuffer) => buffer.length ? buffer.entries[(buffer.start + buffer.length - 1) % LOG_LIMIT] : undefined;

export default function App() {
  const [git, setGit] = useState<GitInfo>({ supported: false });
  const [repositories, setRepositories] = useState<RepositorySummary[]>([]);
  const [selectedId, setSelectedId] = useState<number>();
  const [tab, setTab] = useState<Tab>("changes");
  const [snapshot, setSnapshot] = useState<WorkingTreeSnapshot>();
  const [diff, setDiff] = useState<DiffFile>();
  const [conflict, setConflict] = useState<ConflictDocument & { snapshotId: number }>();
  const [diffMode, setDiffMode] = useState<DiffMode>("unified");
  const [commits, setCommits] = useState<CommitInfo[]>([]);
  const [nextHistoryCursor, setNextHistoryCursor] = useState<HistoryCursor>();
  const [selectedCommit, setSelectedCommit] = useState<string>();
  const [historyLoading, setHistoryLoading] = useState(false);
  const logBuffer = useRef<LogBuffer>(newLogBuffer());
  const [, setLogRevision] = useState(0);
  const [outputOpen, setOutputOpen] = useState(false);
  const [pending, setPending] = useState<Pending>();
  const [dialog, setDialog] = useState<DialogSpec>();
  const [busyOperations, setBusyOperations] = useState<number[]>([]);
  const [toasts, setToasts] = useState<OperationToast[]>([]);
  const [filter, setFilter] = useState("");
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(() => new Set());
  const draggingRepositoryId = useRef<number | undefined>(undefined);
  const [leftWidth, setLeftWidth] = useState(240);
  const [rightWidth, setRightWidth] = useState(360);
  const [outputHeight, setOutputHeight] = useState(190);
  const [language, setLanguage] = useState<Language>("en");
  const [paletteOpen, setPaletteOpen] = useState(false);
  const allowClose = useRef(false);
  const cloneOperations = useRef(new Set<number>());
  const historyRepository = useRef<number | undefined>(undefined);
  const historyRequest = useRef(0);
  const selectedIdRef = useRef<number | undefined>(undefined);
  const statusRequest = useRef(0);
  const repositoryListRequest = useRef(0);
  const repositoryRequests = useRef(new Map<number, number>());
  const streamedSummaries = useRef(new Map<number, RepositorySummary>());
  const operationCallbacks = useRef(new Map<number, OperationFinished>());
  const earlyCompletions = useRef(new Map<number, OperationOutcome>());
  const operationTitles = useRef(new Map<number, string>());
  selectedIdRef.current = selectedId;
  const t = (key: Parameters<typeof translate>[1]) => translate(language, key);
  const showDialog = useCallback((spec: DialogSpec) => setDialog(spec), []);
  const dismissToast = useCallback((id: number) => setToasts((current) => current.filter((toast) => toast.id !== id)), []);

  const selected = repositories.find((repository) => repository.id === selectedId);
  const pushLog = useCallback((kind: LogLine["kind"], message: string) => {
    if (appendLog(logBuffer.current, { id: Date.now() + Math.random(), timestamp: new Date().toISOString(), kind, message })) {
      setLogRevision((current) => current + 1);
    }
  }, []);

  const refreshRepositories = useCallback(async () => {
    const request = ++repositoryListRequest.current;
    streamedSummaries.current.clear();
    try {
      const summaries = await api.refreshRepositories(selectedIdRef.current);
      if (request !== repositoryListRequest.current) return;
      setRepositories(summaries.map((summary) => streamedSummaries.current.get(summary.id) ?? summary));
    }
    catch (error) { if (request === repositoryListRequest.current) { pushLog("error", errorMessage(error)); setOutputOpen(true); } }
  }, [pushLog]);

  const refreshRepository = useCallback(async (repositoryId: number) => {
    const request = (repositoryRequests.current.get(repositoryId) ?? 0) + 1;
    repositoryRequests.current.set(repositoryId, request);
    try {
      const repository = await api.refreshRepository(repositoryId);
      if (repositoryRequests.current.get(repositoryId) !== request) return;
      setRepositories((current) => current.map((item) => item.id === repositoryId ? repository : item));
    } catch (error) {
      if (repositoryRequests.current.get(repositoryId) !== request) return;
      pushLog("error", errorMessage(error)); setOutputOpen(true);
    }
  }, [pushLog]);

  const refreshStatus = useCallback(async (repositoryId = selectedIdRef.current, includeIgnored = false) => {
    if (!repositoryId) return;
    const request = ++statusRequest.current;
    try {
      const value = await api.status(repositoryId, includeIgnored);
      if (request === statusRequest.current && repositoryId === selectedIdRef.current) setSnapshot(value);
    } catch (error) {
      if (request !== statusRequest.current || repositoryId !== selectedIdRef.current) return;
      setSnapshot(undefined); pushLog("error", errorMessage(error)); setOutputOpen(true);
    }
  }, [pushLog]);

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
      historyRepository.current = undefined; pushLog("error", errorMessage(error)); setOutputOpen(true);
    } finally {
      if (request === historyRequest.current && repositoryId === selectedIdRef.current) setHistoryLoading(false);
    }
  }, [pushLog]);

  useEffect(() => {
    api.bootstrap().then((value) => {
      setGit(value.git); setRepositories(value.repositories);
      setSelectedId(value.settings.selectedRepositoryId ?? value.repositories[0]?.id);
      setLeftWidth(value.settings.leftWidth); setRightWidth(value.settings.rightWidth); setOutputHeight(value.settings.outputHeight);
      setLanguage(value.settings.language ?? "en");
    }).catch((error) => { pushLog("error", errorMessage(error)); setOutputOpen(true); });
  }, [pushLog]);

  useEffect(() => { document.documentElement.lang = language; }, [language]);

  useEffect(() => {
    const openPalette = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault(); setPaletteOpen(true);
      }
    };
    window.addEventListener("keydown", openPalette);
    return () => window.removeEventListener("keydown", openPalette);
  }, []);

  useEffect(() => {
    const preventSystemMenu = (event: MouseEvent) => event.preventDefault();
    window.addEventListener("contextmenu", preventSystemMenu);
    return () => window.removeEventListener("contextmenu", preventSystemMenu);
  }, []);

  useEffect(() => {
    if (!selectedId) return;
    setDiff(undefined); setConflict(undefined);
    api.watchRepository(selectedId).catch((error) => pushLog("error", errorMessage(error)));
    if (selected?.kind === "workTree") refreshStatus(selectedId); else { statusRequest.current += 1; setSnapshot(undefined); }
  }, [selectedId, selected?.kind, refreshStatus, pushLog]);

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
      pushLog("error", errorMessage(error)); setOutputOpen(true);
    } finally {
      if (request === historyRequest.current && historyRepository.current === repositoryId) setHistoryLoading(false);
    }
  }, [selectedId, nextHistoryCursor, historyLoading, pushLog]);

  const openCommit = useCallback(async (oid: string) => {
    if (!selectedId) return;
    const repositoryId = selectedId;
    setSelectedCommit(oid);
    setConflict(undefined);
    try {
      const patch = await api.commitDiff(repositoryId, oid);
      if (historyRepository.current === repositoryId) setDiff({ path: t("commitDiff"), staged: false, binary: false, tooLarge: false, patch, hunks: [] });
    }
    catch (error) { pushLog("error", errorMessage(error)); setOutputOpen(true); }
  }, [selectedId, language, pushLog]);

  useEffect(() => {
    const unlisteners = Promise.all([
      listen<OperationEvent>("operation-event", ({ payload }) => {
        pushLog(payload.kind, payload.message);
        if (payload.kind === "started") {
          operationTitles.current.set(payload.operationId, payload.message);
          if (payload.repositoryId == null) cloneOperations.current.add(payload.operationId);
          setBusyOperations((ids) => ids.includes(payload.operationId) ? ids : [...ids, payload.operationId]);
        }
        if (payload.kind === "finished") {
          const outcome = payload.outcome ?? "failed";
          const title = operationTitles.current.get(payload.operationId) ?? "Git";
          operationTitles.current.delete(payload.operationId);
          setToasts((current) => [...current, { id: payload.operationId, title, message: payload.message, outcome }].slice(-3));
          const callback = operationCallbacks.current.get(payload.operationId);
          operationCallbacks.current.delete(payload.operationId);
          if (callback) callback(outcome);
          else {
            earlyCompletions.current.set(payload.operationId, outcome);
            if (earlyCompletions.current.size > 20) earlyCompletions.current.delete(earlyCompletions.current.keys().next().value!);
          }
          setBusyOperations((ids) => ids.filter((id) => id !== payload.operationId));
          if (payload.outcome !== "succeeded") setOutputOpen(true);
          if (cloneOperations.current.delete(payload.operationId)) {
            refreshRepositories();
            if (payload.outcome === "succeeded" && payload.repositoryId) setSelectedId(payload.repositoryId);
          }
        }
        if (payload.kind === "stderr") setOutputOpen(true);
      }),
      listen<{ repositoryId: number }>("repository-changed", ({ payload }) => {
        refreshRepository(payload.repositoryId);
        if (payload.repositoryId === selectedIdRef.current) refreshStatus(payload.repositoryId);
      }),
      listen<RepositorySummary>("repository-summary-refreshed", ({ payload }) => {
        streamedSummaries.current.set(payload.id, payload);
        setRepositories((current) => current.map((repository) => repository.id === payload.id ? payload : repository));
      }),
      listen("repository-list-changed", refreshRepositories),
    ]);
    return () => { unlisteners.then((values) => values.forEach((unlisten) => unlisten())); };
  }, [pushLog, refreshRepositories, refreshRepository, refreshStatus]);

  useEffect(() => {
    const listener = getCurrentWindow().onCloseRequested(async (event) => {
      if (allowClose.current || !busyOperations.length) return;
      event.preventDefault();
      showDialog({
        title: t("confirm"), message: `${busyOperations.length} ${t("closeOperations")}`, danger: true,
        onSubmit: async () => {
          await Promise.allSettled(busyOperations.map(api.cancel));
          allowClose.current = true;
          await getCurrentWindow().close();
        },
      });
    });
    return () => { listener.then((unlisten) => unlisten()); };
  }, [busyOperations, language, showDialog]);

  useEffect(() => {
    const closeMenu = () => document.querySelector<HTMLDivElement>(".row-menu-popover:popover-open")?.hidePopover();
    window.addEventListener("blur", closeMenu);
    return () => window.removeEventListener("blur", closeMenu);
  }, []);

  const startOperation = useCallback(async (repositoryId: number, request: OperationRequest, confirmed: boolean, onFinished?: OperationFinished) => {
    const result = await api.start(repositoryId, request, confirmed);
    const finished = onFinished || request.type === "commit" ? (outcome: OperationOutcome) => {
      if (outcome === "succeeded" && request.type === "commit" && historyRepository.current === repositoryId) {
        if (selectedIdRef.current === repositoryId) void refreshHistory(repositoryId); else historyRepository.current = undefined;
      }
      onFinished?.(outcome);
    } : undefined;
    if (finished) {
      const outcome = earlyCompletions.current.get(result.operationId);
      if (outcome) { earlyCompletions.current.delete(result.operationId); finished(outcome); }
      else operationCallbacks.current.set(result.operationId, finished);
    }
  }, [refreshHistory]);

  const run = useCallback(async (request: OperationRequest, onFinished?: OperationFinished) => {
    if (!selectedId) { onFinished?.("failed"); return; }
    try {
      const preview = await api.preview(selectedId, request);
      if (preview.requiresConfirmation) { setPending({ repositoryId: selectedId, request, preview, onFinished }); return; }
      await startOperation(selectedId, request, false, onFinished);
    } catch (error) { onFinished?.("failed"); pushLog("error", errorMessage(error)); setOutputOpen(true); }
  }, [selectedId, pushLog, startOperation]);

  const confirmPending = async () => {
    if (!pending) return;
    try { await startOperation(pending.repositoryId, pending.request, true, pending.onFinished); setPending(undefined); }
    catch (error) { pending.onFinished?.("failed"); pushLog("error", errorMessage(error)); setOutputOpen(true); }
  };

  const chooseDirectory = async () => {
    const path = await open({ directory: true, multiple: false });
    return typeof path === "string" ? path : undefined;
  };
  const register = async () => { const path = await chooseDirectory(); if (path) await mutateRepository(() => api.addRepository(path)); };
  const initialize = async () => { const path = await chooseDirectory(); if (path) await mutateRepository(() => api.initRepository(path)); };
  const clone = () => showDialog({ title: t("clone"), submitLabel: t("clone"), fields: [{ name: "url", label: t("remoteUrl"), required: true }], onSubmit: async ({ url }) => {
    const destination = await chooseDirectory();
    if (!destination) return;
    try {
      await api.cloneRepository(String(url).trim(), destination);
    } catch (error) { pushLog("error", errorMessage(error)); setOutputOpen(true); }
  } });
  const mutateRepository = async (action: () => Promise<RepositorySummary>) => {
    try { const repository = await action(); await refreshRepositories(); setSelectedId(repository.id); }
    catch (error) { pushLog("error", errorMessage(error)); setOutputOpen(true); }
  };

  const updateSelected = async (changes: Partial<{ name: string; group: string; favorite: boolean }>) => {
    if (!selected) return;
    try {
      await api.updateRepository({ id: selected.id, path: selected.path, name: changes.name ?? selected.name, group: changes.group ?? selected.group, favorite: changes.favorite ?? selected.favorite, order: selected.order });
      await refreshRepositories();
    } catch (error) { pushLog("error", errorMessage(error)); setOutputOpen(true); }
  };

  const relocateSelected = async () => { const path = await chooseDirectory(); if (path && selectedId) await mutateRepository(() => api.relocateRepository(selectedId, path)); };
  const removeSelected = () => { if (selected) showDialog({ title: t("removeGitDock"), message: `${selected.name} ${t("removeRepositoryConfirm")}`, danger: true, onSubmit: async () => {
    await api.removeRepository(selected.id); setSelectedId(repositories.find((item) => item.id !== selected.id)?.id); await refreshRepositories();
  } }); };
  const selectGit = async () => {
    const path = await open({ directory: false, multiple: false, title: t("selectGit") });
    if (typeof path === "string") setGit(await api.setGitPath(path));
  };
  const forcePush = () => {
    if (!selectedId || !selected?.branch) return;
    showDialog({ title: t("forcePush"), submitLabel: t("forcePush"), fields: [{ name: "remote", label: t("remote"), value: "origin", required: true }], onSubmit: async ({ remote }) => {
      try {
        const value = String(remote).trim();
        const branches = await api.branches(selectedId);
        const expectedOid = branches.find((branch) => branch.remote && branch.name === `${value}/${selected.branch}`)?.oid;
        if (!expectedOid) throw new Error(`${t("fetch")} ${value}/${selected.branch} ${t("fetchBeforeForce")}`);
        await run({ type: "forcePushWithLease", remote: value, branch: selected.branch, expectedOid });
      } catch (error) { pushLog("error", errorMessage(error)); setOutputOpen(true); }
    } });
  };

  const toggleLanguage = () => {
    const next = language === "en" ? "zh-CN" : "en";
    setLanguage(next);
    api.saveLanguage(next).catch((error) => { pushLog("error", errorMessage(error)); setOutputOpen(true); });
  };

  const beginResize = (side: "left" | "right" | "output", event: React.PointerEvent) => {
    event.preventDefault();
    const start = side === "output" ? event.clientY : event.clientX;
    const initial = side === "left" ? leftWidth : side === "right" ? rightWidth : outputHeight;
    let current = initial;
    const move = (next: PointerEvent) => {
      const delta = side === "output" ? start - next.clientY : next.clientX - start;
      current = Math.max(side === "left" ? 190 : side === "right" ? 300 : 120, Math.min(side === "left" ? 420 : side === "right" ? 560 : 420, initial + (side === "right" ? -delta : delta)));
      if (side === "left") setLeftWidth(current); else if (side === "right") setRightWidth(current); else setOutputHeight(current);
    };
    const stop = () => {
      window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", stop);
      api.saveLayout(side === "left" ? current : leftWidth, side === "right" ? current : rightWidth, side === "output" ? current : outputHeight).catch((error) => pushLog("error", errorMessage(error)));
    };
    window.addEventListener("pointermove", move); window.addEventListener("pointerup", stop, { once: true });
  };

  const openDiff = useCallback(async (file: FileChange, staged: boolean) => {
    if (!selectedId || !snapshot) return;
    const repositoryId = selectedId;
    const snapshotId = snapshot.id;
    try {
      if (file.conflict) {
        const document = await api.conflictDocument(repositoryId, snapshotId, file.path);
        if (selectedIdRef.current === repositoryId) { setConflict({ ...document, snapshotId }); setDiff(undefined); }
      } else {
        const nextDiff = await api.diff(repositoryId, snapshotId, file.path, staged);
        if (selectedIdRef.current === repositoryId) { setDiff(nextDiff); setConflict(undefined); }
      }
    }
    catch (error) { pushLog("error", errorMessage(error)); setOutputOpen(true); }
  }, [selectedId, snapshot, pushLog]);

  const selectRepository = useCallback((repositoryId: number) => setSelectedId(repositoryId), []);
  const closeDiff = useCallback(() => { setDiff(undefined); setConflict(undefined); }, []);
  const openRepositoryFile = useCallback((path: string) => {
    if (selectedId) api.openRepositoryFile(selectedId, path).catch((error) => pushLog("error", errorMessage(error)));
  }, [selectedId, pushLog]);
  const loadIgnored = useCallback(() => refreshStatus(selectedId, true), [selectedId, refreshStatus]);
  const reportError = useCallback((message: string) => pushLog("error", message), [pushLog]);
  const showBranchDiff = useCallback((value: string) => { setConflict(undefined); setDiff({ path: translate(language, "branchComparison"), staged: false, binary: false, tooLarge: false, patch: value, hunks: [] }); }, [language]);
  const resolveConflict = useCallback((choices: ConflictResolution[]) => {
    if (!conflict) return;
    const document = conflict;
    void run({ type: "resolveConflictBlocks", snapshotId: document.snapshotId, documentId: document.id, path: document.path, choices }, (outcome) => {
      if (outcome === "succeeded") setConflict((current) => current?.id === document.id ? undefined : current);
    });
  }, [conflict, run]);

  const repositoryGroups = useMemo<RepositoryGroup[]>(() => {
    const query = filter.trim().toLowerCase();
    const visible = repositories
      .filter((repository) => `${repository.name} ${repository.path} ${repository.group ?? ""}`.toLowerCase().includes(query))
      .sort((left, right) => left.order - right.order || left.name.localeCompare(right.name));
    const favorites = visible.filter((repository) => repository.favorite);
    const grouped = new Map<string, RepositorySummary[]>();
    for (const repository of visible.filter((item) => !item.favorite)) {
      const key = repository.group?.trim() || UNGROUPED_GROUP;
      grouped.set(key, [...(grouped.get(key) ?? []), repository]);
    }
    const named = [...grouped.entries()]
      .filter(([key]) => key !== UNGROUPED_GROUP)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, items]) => ({ key, label: key, repositories: items }));
    return [
      { key: FAVORITES_GROUP, label: t("favorites"), repositories: favorites },
      ...named,
      { key: UNGROUPED_GROUP, label: t("ungrouped"), repositories: grouped.get(UNGROUPED_GROUP) ?? [] },
    ];
  }, [repositories, filter, language]);

  const persistRepositoryLayout = useCallback(async (ordered: RepositorySummary[]) => {
    const previous = repositories;
    const next = ordered.map((repository, order) => ({ ...repository, order }));
    setRepositories(next);
    try {
      await api.reorderRepositories(next.map(({ id, group, favorite, order }) => ({ id, group, favorite, order })));
    } catch (error) {
      setRepositories(previous); pushLog("error", errorMessage(error)); setOutputOpen(true);
    }
  }, [repositories, pushLog]);

  const moveRepository = useCallback((repositoryId: number, targetGroup: string, targetId?: number) => {
    if (filter.trim() || repositoryId === targetId) return;
    const groups = repositoryGroups.map((group) => ({ ...group, repositories: [...group.repositories] }));
    let moving: RepositorySummary | undefined;
    for (const group of groups) {
      const index = group.repositories.findIndex((repository) => repository.id === repositoryId);
      if (index >= 0) moving = group.repositories.splice(index, 1)[0];
    }
    const target = groups.find((group) => group.key === targetGroup);
    if (!moving || !target) return;
    moving = targetGroup === FAVORITES_GROUP
      ? { ...moving, favorite: true }
      : { ...moving, favorite: false, group: targetGroup === UNGROUPED_GROUP ? undefined : targetGroup };
    const index = targetId ? target.repositories.findIndex((repository) => repository.id === targetId) : -1;
    target.repositories.splice(index < 0 ? target.repositories.length : index, 0, moving);
    void persistRepositoryLayout(groups.flatMap((group) => group.repositories));
  }, [filter, repositoryGroups, persistRepositoryLayout]);

  const moveRepositoryBy = useCallback((repositoryId: number, direction: -1 | 1) => {
    if (filter.trim()) return;
    const groups = repositoryGroups.map((group) => ({ ...group, repositories: [...group.repositories] }));
    const group = groups.find((item) => item.repositories.some((repository) => repository.id === repositoryId));
    if (!group) return;
    const index = group.repositories.findIndex((repository) => repository.id === repositoryId);
    const target = index + direction;
    if (target < 0 || target >= group.repositories.length) return;
    [group.repositories[index], group.repositories[target]] = [group.repositories[target], group.repositories[index]];
    void persistRepositoryLayout(groups.flatMap((item) => item.repositories));
  }, [filter, repositoryGroups, persistRepositoryLayout]);

  const updateGroup = useCallback((group: string, replacement?: string) => {
    const ordered = [...repositories].sort((left, right) => left.order - right.order).map((repository) => repository.group === group ? { ...repository, group: replacement } : repository);
    void persistRepositoryLayout(ordered);
  }, [repositories, persistRepositoryLayout]);

  const acceptRepositoryDrop = useCallback((event: React.DragEvent) => {
    if (filter.trim()) return;
    event.preventDefault(); event.dataTransfer.dropEffect = "move";
  }, [filter]);

  const exportLogs = useCallback(async () => {
    try {
      const stamp = new Date().toISOString().replaceAll(":", "-").replace(/\.\d{3}Z$/, "Z");
      await api.exportSessionLog(`gitdock-session-${stamp}.log`, readLogs(logBuffer.current).map(({ timestamp, kind, message }) => ({ timestamp, kind, message })));
    } catch (error) { pushLog("error", errorMessage(error)); setOutputOpen(true); }
  }, [pushLog]);

  const clearLogs = () => { logBuffer.current = newLogBuffer(); setLogRevision((current) => current + 1); };
  const logCount = logBuffer.current.length;

  const command = (id: string, key: Parameters<typeof translate>[1], action: () => void, enabled = true): CommandItem | undefined => enabled ? { id, label: t(key), search: `${t(key)} ${translate("en", key)}`.toLowerCase(), action } : undefined;
  const commands = [
    command("changes", "changes", () => { setTab("changes"); closeDiff(); }, Boolean(selected)),
    command("history", "history", () => { setTab("history"); closeDiff(); }, Boolean(selected)),
    command("branches", "branches", () => { setTab("branches"); closeDiff(); }, Boolean(selected)),
    command("stashes", "stashes", () => { setTab("stashes"); closeDiff(); }, Boolean(selected)),
    command("add", "addRepository", register, git.supported), command("clone", "clone", clone, git.supported), command("init", "initialize", initialize, git.supported),
    command("refresh", "refreshAll", refreshRepositories), command("language", "language", toggleLanguage), command("git", "selectGit", selectGit),
    command("fetch", "fetch", () => { void run({ type: "fetch", prune: false }); }, Boolean(selected?.capabilities.canManageRemotes)),
    command("pull", "pull", () => { void run({ type: "pull" }); }, Boolean(selected?.capabilities.canWriteWorkTree)),
    command("pull-merge", "pullMerge", () => { void run({ type: "pull", strategy: "merge" }); }, Boolean(selected?.capabilities.canWriteWorkTree)),
    command("pull-rebase", "pullRebase", () => { void run({ type: "pull", strategy: "rebase" }); }, Boolean(selected?.capabilities.canWriteWorkTree)),
    command("pull-ff", "pullFf", () => { void run({ type: "pull", strategy: "fastForwardOnly" }); }, Boolean(selected?.capabilities.canWriteWorkTree)),
    command("push", "push", () => { void run({ type: "push" }); }, Boolean(selected?.capabilities.canManageRemotes)),
    command("force-push", "forcePush", forcePush, Boolean(selected?.capabilities.canManageRemotes && selected.branch)),
    command("upstream", "setUpstream", () => showDialog({ title: t("setUpstream"), fields: [{ name: "remote", label: t("remote"), value: "origin", required: true }], onSubmit: ({ remote }) => { if (selected?.branch) run({ type: "setUpstream", remote: String(remote).trim(), branch: selected.branch }); } }), Boolean(selected?.branch)),
    command("cherry-pick", "cherryPickCommits", () => showDialog({ title: t("cherryPickCommits"), fields: [{ name: "commits", label: t("commitOids"), required: true }], onSubmit: ({ commits }) => run({ type: "cherryPick", commits: String(commits).trim().split(/\s+/) }) }), Boolean(selected?.capabilities.canWriteWorkTree)),
    command("undo", "undoCommit", () => { void run({ type: "undoLastCommit" }); }, Boolean(selected?.capabilities.canWriteWorkTree)),
    command("rename", "renameEntry", () => showDialog({ title: t("renameEntry"), fields: [{ name: "name", label: t("repositoryName"), value: selected?.name, required: true }], onSubmit: ({ name }) => updateSelected({ name: String(name).trim() }) }), Boolean(selected)),
    command("favorite", selected?.favorite ? "removeFavorite" : "addFavorite", () => { void updateSelected({ favorite: !selected?.favorite }); }, Boolean(selected)),
    command("group", "setGroup", () => showDialog({ title: t("setGroup"), fields: [{ name: "group", label: t("group"), value: selected?.group ?? "" }], onSubmit: ({ group }) => updateSelected({ group: String(group).trim() }) }), Boolean(selected)),
    command("relocate", "relocate", relocateSelected, Boolean(selected)), command("remove", "removeGitDock", removeSelected, Boolean(selected)),
  ].filter((item): item is CommandItem => Boolean(item));

  const outputPanel = <section className={`output-panel ${outputOpen ? "open" : ""}`}>
    <button className="output-handle" onClick={() => setOutputOpen((value) => !value)}><span>{t("gitOutput")}</span><span>{busyOperations.length ? `${busyOperations.length} ${t("running")}` : `${logCount} ${t("lines")}`} {outputOpen ? "⌄" : "⌃"}</span></button>
    {outputOpen && <><div className="resize-handle resize-output" onPointerDown={(event) => beginResize("output", event)} /><div className="log" style={{ height: outputHeight }}><div className="log-toolbar">{busyOperations.map((id) => <button key={id} onClick={() => api.cancel(id)}>{t("cancel")} #{id}</button>)}<button disabled={!logCount} onClick={exportLogs}>{t("exportLog")}</button><button onClick={clearLogs}>{t("clear")}</button></div>{readLogs(logBuffer.current).map((line) => <div key={line.id} className={`log-${line.kind}`}><time>{line.timestamp}</time> {line.message}</div>)}</div></>}
  </section>;
  const toastStack = <ToastStack toasts={toasts} onDismiss={dismissToast} />;

  if (!repositories.length) return <I18nProvider language={language}><><main className="empty-workspace"><EmptyState git={git} onAdd={register} onClone={clone} onInit={initialize} onSelectGit={selectGit} onToggleLanguage={toggleLanguage} lastLog={lastLog(logBuffer.current)} />{outputPanel}</main>{toastStack}{paletteOpen && <CommandPalette items={commands} onClose={() => setPaletteOpen(false)} />}{dialog && <FormDialog spec={dialog} onClose={() => setDialog(undefined)} />}</></I18nProvider>;

  return (
    <I18nProvider language={language}><div className="app-shell" style={{ gridTemplateColumns: `${leftWidth}px 1fr` }}>
      <aside className="repo-sidebar">
        <header className="brand"><RailMark /><div><strong>GitDock</strong><span>{git.supported ? `Git ${git.version}` : t("gitUnavailable")}</span></div></header>
        <label className="search"><span>⌕</span><input aria-label={t("searchRepositories")} placeholder={t("findRepository")} value={filter} onChange={(event) => setFilter(event.target.value)} /></label>
        <div className="repo-list" role="listbox" aria-label={t("repositories")}>
          {repositoryGroups.map((group) => <section role="group" aria-label={group.label} className={`repo-group ${!collapsedGroups.has(group.key) && !group.repositories.length ? "empty" : ""}`} key={group.key} onDragEnter={acceptRepositoryDrop} onDragOver={acceptRepositoryDrop} onDrop={(event) => { event.preventDefault(); const repositoryId = Number(event.dataTransfer.getData("text/plain")) || draggingRepositoryId.current; if (repositoryId) moveRepository(repositoryId, group.key); draggingRepositoryId.current = undefined; }}>
            <header><button aria-expanded={!collapsedGroups.has(group.key)} onClick={() => setCollapsedGroups((current) => { const next = new Set(current); if (next.has(group.key)) next.delete(group.key); else next.add(group.key); return next; })}><span>{collapsedGroups.has(group.key) ? "▸" : "▾"} {group.label}</span><code>{group.repositories.length}</code></button>{group.key !== FAVORITES_GROUP && group.key !== UNGROUPED_GROUP && <RowMenu><button onClick={() => showDialog({ title: t("renameGroup"), fields: [{ name: "group", label: t("group"), value: group.label, required: true }], onSubmit: ({ group: value }) => updateGroup(group.key, String(value).trim()) })}>{t("rename")}</button><button onClick={() => updateGroup(group.key)}>{t("ungroup")}</button></RowMenu>}</header>
            {!collapsedGroups.has(group.key) && group.repositories.map((repository, index) => <MemoRepositoryRow key={repository.id} repository={repository} selected={repository.id === selectedId} draggable={!filter.trim()} canMoveUp={!filter.trim() && index > 0} canMoveDown={!filter.trim() && index < group.repositories.length - 1} onSelect={selectRepository} onMove={(direction) => moveRepositoryBy(repository.id, direction)} onDragStart={(event) => { event.dataTransfer.effectAllowed = "move"; event.dataTransfer.setData("text/plain", String(repository.id)); draggingRepositoryId.current = repository.id; }} onDragEnd={() => { draggingRepositoryId.current = undefined; }} onDrop={(event) => { event.preventDefault(); event.stopPropagation(); const repositoryId = Number(event.dataTransfer.getData("text/plain")) || draggingRepositoryId.current; if (repositoryId) moveRepository(repositoryId, group.key, repository.id); draggingRepositoryId.current = undefined; }} />)}
          </section>)}
        </div>
        <footer className="sidebar-actions"><button onClick={register}>{t("add")}</button><button onClick={clone}>{t("clone")}</button><button onClick={initialize}>{t("init")}</button><button onClick={toggleLanguage}>{t("language")}</button></footer><div className="resize-handle resize-left" onPointerDown={(event) => beginResize("left", event)} />
      </aside>

      <main className="workspace">
        <header className="topbar">
          <div className="repo-context"><span className="branch-dot" /> <strong>{selected?.name}</strong><code>{selected?.branch || shortOid(selected?.headOid)}</code></div>
          <nav aria-label={t("workflows")}>{(["changes", "history", "branches", "stashes"] as Tab[]).map((item) => <button key={item} className={tab === item ? "active" : ""} onClick={() => { setTab(item); closeDiff(); }}>{t(item)}</button>)}</nav>
          <div className="sync-actions"><button aria-label={t("commandPalette")} onClick={() => setPaletteOpen(true)}>⌘K</button><button disabled={!selected?.capabilities.canManageRemotes} onClick={() => run({ type: "fetch", prune: false })}>{t("fetch")}</button><button disabled={!selected?.capabilities.canWriteWorkTree} onClick={() => run({ type: "pull" })}>{t("pull")}</button><button className="primary" disabled={!selected?.capabilities.canManageRemotes} onClick={() => run({ type: "push" })}>{t("push")}</button><RowMenu label={t("more")}><button onClick={refreshRepositories}>{t("refreshAll")}</button><button onClick={() => run({ type: "pull", strategy: "merge" })}>{t("pullMerge")}</button><button onClick={() => run({ type: "pull", strategy: "rebase" })}>{t("pullRebase")}</button><button onClick={() => run({ type: "pull", strategy: "fastForwardOnly" })}>{t("pullFf")}</button><button onClick={forcePush}>{t("forcePush")}</button><button onClick={() => showDialog({ title: t("setUpstream"), fields: [{ name: "remote", label: t("remote"), value: "origin", required: true }], onSubmit: ({ remote }) => { if (selected?.branch) run({ type: "setUpstream", remote: String(remote).trim(), branch: selected.branch }); } })}>{t("setUpstream")}</button><button onClick={() => showDialog({ title: t("cherryPickCommits"), fields: [{ name: "commits", label: t("commitOids"), required: true }], onSubmit: ({ commits }) => run({ type: "cherryPick", commits: String(commits).trim().split(/\s+/) }) })}>{t("cherryPickCommits")}</button><button onClick={() => run({ type: "undoLastCommit" })}>{t("undoCommit")}</button><button onClick={() => showDialog({ title: t("renameEntry"), fields: [{ name: "name", label: t("repositoryName"), value: selected?.name, required: true }], onSubmit: ({ name }) => updateSelected({ name: String(name).trim() }) })}>{t("renameEntry")}</button><button onClick={() => updateSelected({ favorite: !selected?.favorite })}>{selected?.favorite ? t("removeFavorite") : t("addFavorite")}</button><button onClick={() => showDialog({ title: t("setGroup"), fields: [{ name: "group", label: t("group"), value: selected?.group ?? "" }], onSubmit: ({ group }) => updateSelected({ group: String(group).trim() }) })}>{t("setGroup")}</button><button onClick={relocateSelected}>{t("relocate")}</button><button onClick={selectGit}>{t("selectGit")}</button><button className="menu-danger" onClick={removeSelected}>{t("removeGitDock")}</button></RowMenu></div>
        </header>

        <div className="work-area" style={{ gridTemplateColumns: `minmax(480px, 1fr) ${rightWidth}px` }}>
          <section className="canvas">
            {conflict ? <ConflictEditor key={conflict.id} document={conflict} onBack={closeDiff} onResolve={resolveConflict} /> : diff ? <MemoDiffView diff={diff} snapshotId={snapshot?.id} mode={diffMode} onModeChange={setDiffMode} onBack={closeDiff} onRun={run} /> : tab === "changes" ? <MemoChangesOverview repository={selected} snapshot={snapshot} /> : tab === "history" ? <MemoHistoryCanvas commits={commits} selectedOid={selectedCommit} onSelect={openCommit} /> : tab === "branches" ? <BranchCanvas repository={selected} /> : <StashCanvas repository={selected} />}
          </section>
          <aside className="tool-pane"><div className="resize-handle resize-right" onPointerDown={(event) => beginResize("right", event)} />
            {tab === "changes" && <MemoChangesPane repository={selected} snapshot={snapshot} onOpen={openDiff} onOpenExternal={openRepositoryFile} onLoadIgnored={loadIgnored} onRun={run} />}
            {tab === "history" && <MemoHistoryPane commits={commits} selectedOid={selectedCommit} loading={historyLoading} hasMore={historyRepository.current === selectedId && nextHistoryCursor !== undefined} onLoadMore={loadMoreHistory} onSelect={openCommit} onRun={run} />}
            {tab === "branches" && <MemoBranchesPane repositoryId={selectedId!} onRun={run} onDialog={showDialog} onDiff={showBranchDiff} onError={reportError} />}
            {tab === "stashes" && <MemoStashesPane repositoryId={selectedId!} onRun={run} onDialog={showDialog} onError={reportError} />}
          </aside>
        </div>

        {outputPanel}
      </main>

      {pending && <ConfirmDialog pending={pending} onCancel={() => { pending.onFinished?.("cancelled"); setPending(undefined); }} onConfirm={confirmPending} />}
      {toastStack}
      {paletteOpen && <CommandPalette items={commands} onClose={() => setPaletteOpen(false)} />}
      {dialog && <FormDialog spec={dialog} onClose={() => setDialog(undefined)} />}
    </div></I18nProvider>
  );
}

function ToastStack({ toasts, onDismiss }: { toasts: OperationToast[]; onDismiss: (id: number) => void }) {
  return <div className="toast-stack">{toasts.map((toast) => <OperationToastView key={toast.id} toast={toast} onDismiss={onDismiss} />)}</div>;
}

function OperationToastView({ toast, onDismiss }: { toast: OperationToast; onDismiss: (id: number) => void }) {
  const { t } = useI18n();
  useEffect(() => { const timer = window.setTimeout(() => onDismiss(toast.id), 3_000); return () => window.clearTimeout(timer); }, [toast.id, onDismiss]);
  const result = toast.outcome === "succeeded" ? t("operationSucceeded") : toast.outcome === "cancelled" ? t("operationCancelled") : t("operationFailed");
  return <div className={`operation-toast toast-${toast.outcome}`} role={toast.outcome === "failed" ? "alert" : "status"}><span><strong>{toast.title}</strong><small>{result}{toast.outcome === "failed" && toast.message ? ` · ${toast.message}` : ""}</small></span><button aria-label={t("dismissNotification")} onClick={() => onDismiss(toast.id)}>×</button></div>;
}

function EmptyState({ git, onAdd, onClone, onInit, onSelectGit, onToggleLanguage, lastLog }: { git: GitInfo; onAdd: () => void; onClone: () => void; onInit: () => void; onSelectGit: () => void; onToggleLanguage: () => void; lastLog?: LogLine }) {
  const { t } = useI18n();
  return <main className="empty-state"><div className="empty-brand"><RailMark /><span>GITDOCK / WORKSPACE</span></div><h1>{t("emptyTitle1")}<br />{t("emptyTitle2")}</h1><p>{t("emptyDescription")}</p><div className="empty-actions"><button className="primary" disabled={!git.supported} onClick={onAdd}>{t("addRepository")}</button><button disabled={!git.supported} onClick={onClone}>{t("clone")}</button><button disabled={!git.supported} onClick={onInit}>{t("initialize")}</button>{!git.supported && <button onClick={onSelectGit}>{t("selectGit")}</button>}<button onClick={onToggleLanguage}>{t("language")}</button></div><div className={`git-check ${git.supported ? "ok" : "bad"}`}><span>{git.supported ? "●" : "×"}</span><div><strong>{git.supported ? `Git ${git.version}` : t("gitRequired")}</strong><small>{git.path ?? git.error}</small></div></div>{lastLog && <p className="empty-error">{lastLog.message}</p>}</main>;
}

function RailMark() { return <svg className="rail-mark" viewBox="0 0 32 32" aria-hidden="true"><path d="M9 4v18a6 6 0 0 0 6 6h3" /><path d="M23 4v7a5 5 0 0 1-5 5H9" /><circle cx="9" cy="4" r="2.5" /><circle cx="23" cy="4" r="2.5" /><circle cx="20" cy="28" r="2.5" /></svg>; }

function RepositoryRow({ repository, selected, draggable, canMoveUp, canMoveDown, onSelect, onMove, onDragStart, onDragEnd, onDrop }: { repository: RepositorySummary; selected: boolean; draggable: boolean; canMoveUp: boolean; canMoveDown: boolean; onSelect: (repositoryId: number) => void; onMove: (direction: -1 | 1) => void; onDragStart: React.DragEventHandler<HTMLDivElement>; onDragEnd: () => void; onDrop: React.DragEventHandler<HTMLDivElement> }) {
  const { t } = useI18n();
  const state = repository.kind === "missing" ? "missing" : repository.conflictCount ? "conflict" : repository.changedCount ? "changed" : "clean";
  return <div role="option" tabIndex={0} aria-selected={selected} className="repo-row-shell" draggable={draggable} onClick={() => onSelect(repository.id)} onKeyDown={(event) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); onSelect(repository.id); } }} onDragStart={onDragStart} onDragEnd={onDragEnd} onDragOver={(event) => { if (draggable) { event.preventDefault(); event.dataTransfer.dropEffect = "move"; } }} onDrop={onDrop}><button className={`repo-row ${selected ? "selected" : ""}`} tabIndex={-1}><span className={`status-rail ${state}`} /><span className="repo-copy"><span className="repo-name">{repository.favorite && "★ "}{repository.name}<i>{repository.conflictCount ? `${repository.conflictCount} ${t("conflicts")}` : t(state)}</i></span><span className="repo-meta"><code>{repository.branch || shortOid(repository.headOid)}</code><span>{repository.changedCount ? `±${repository.changedCount}` : t("clean")}</span>{(repository.ahead || repository.behind) ? <span>↑{repository.ahead} ↓{repository.behind}</span> : null}</span></span></button><RowMenu><button disabled={!canMoveUp} onClick={() => onMove(-1)}>{t("moveUp")}</button><button disabled={!canMoveDown} onClick={() => onMove(1)}>{t("moveDown")}</button></RowMenu></div>;
}

function ChangesOverview({ repository, snapshot }: { repository?: RepositorySummary; snapshot?: WorkingTreeSnapshot }) {
  const { t } = useI18n();
  const changed = snapshot?.files.filter((file) => !file.ignored).length ?? 0;
  if (!changed) return <div className="canvas-empty"><span className="large-check">✓</span><h2>{t("workingTreeClean")}</h2><p>{repository?.lastCommit || t("noLocalChanges")}</p></div>;
  return <div className="canvas-empty"><div className="change-tally"><strong>{changed}</strong><span>{t("workingTreeChanges")}</span></div><h2>{t("selectFile")}</h2><p>{t("inspectHint")}</p></div>;
}

function ChangesPane({ repository, snapshot, onOpen, onOpenExternal, onLoadIgnored, onRun }: { repository?: RepositorySummary; snapshot?: WorkingTreeSnapshot; onOpen: (file: FileChange, staged: boolean) => void; onOpenExternal: (path: string) => void; onLoadIgnored: () => void; onRun: RunOperation }) {
  const { t } = useI18n();
  const [messages, setMessages] = useState<Record<number, string>>({}); const [amend, setAmend] = useState(false); const [signoff, setSignoff] = useState(false);
  const [committingRepositories, setCommittingRepositories] = useState<Set<number>>(() => new Set());
  const [stageSelection, setStageSelection] = useState<string[]>([]); const [unstageSelection, setUnstageSelection] = useState<string[]>([]);
  const repositoryId = repository?.id;
  const message = repositoryId ? messages[repositoryId] ?? "" : "";
  const commitRunning = repositoryId ? committingRepositories.has(repositoryId) : false;
  const files = snapshot?.files ?? [];
  const groups = [
    [t("conflictsGroup"), files.filter((f) => f.conflict), "conflict"],
    [t("staged"), files.filter((f) => f.staged && !f.conflict), "staged"],
    [t("unstaged"), files.filter((f) => f.unstaged && !f.conflict && f.kind !== "Untracked" && !f.ignored), "unstaged"],
    [t("untracked"), files.filter((f) => f.kind === "Untracked"), "untracked"],
    [t("ignored"), files.filter((f) => f.ignored), "ignored"],
  ] as const;
  const validStagePaths = new Set(groups.filter(([, , type]) => type === "unstaged" || type === "untracked").flatMap(([, entries]) => entries.map((file) => file.path)));
  const validUnstagePaths = new Set(groups.filter(([, , type]) => type === "staged").flatMap(([, entries]) => entries.map((file) => file.path)));
  useEffect(() => {
    setStageSelection((current) => current.filter((path) => validStagePaths.has(path)));
    setUnstageSelection((current) => current.filter((path) => validUnstagePaths.has(path)));
  }, [snapshot?.id, repository?.id]);
  const toggle = (path: string, selected: string[], setSelected: React.Dispatch<React.SetStateAction<string[]>>) => setSelected(selected.includes(path) ? selected.filter((item) => item !== path) : [...selected, path]);
  const batch = (type: "stageFiles" | "unstageFiles", paths: string[], clear: () => void) => { onRun({ type, paths }); clear(); };
  const commit = (event: React.FormEvent) => {
    event.preventDefault();
    if (!repositoryId) return;
    const submittedMessage = message;
    const submittedRepositoryId = repositoryId;
    setCommittingRepositories((current) => new Set(current).add(submittedRepositoryId));
    onRun({ type: "commit", message, amend, signoff }, (outcome) => {
      setCommittingRepositories((current) => { const next = new Set(current); next.delete(submittedRepositoryId); return next; });
      if (outcome === "succeeded") setMessages((current) => current[submittedRepositoryId] === submittedMessage ? { ...current, [submittedRepositoryId]: "" } : current);
    });
  };
  return <div className="changes-pane"><div className="pane-title"><span>{t("workingTree")}</span><span className="batch-actions">{stageSelection.length > 0 && <button onClick={() => batch("stageFiles", stageSelection, () => setStageSelection([]))}>{t("stageSelected")} ({stageSelection.length})</button>}{unstageSelection.length > 0 && <button onClick={() => batch("unstageFiles", unstageSelection, () => setUnstageSelection([]))}>{t("unstageSelected")} ({unstageSelection.length})</button>}{stageSelection.length === 0 && unstageSelection.length === 0 && <button onClick={onLoadIgnored}>{t("loadIgnored")}</button>}</span></div><div className="change-groups">{repository?.ongoing && <div className="ongoing"><strong>{repository.ongoing.kind} {t("inProgress")}</strong>{repository.ongoing.canContinue && <button onClick={() => onRun({ type: "continue", kind: repository.ongoing!.kind })}>{t("continue")}</button>}{repository.ongoing.canSkip && <button onClick={() => onRun({ type: "skip", kind: repository.ongoing!.kind })}>{t("skip")}</button>}{repository.ongoing.canAbort && <button onClick={() => onRun({ type: "abort", kind: repository.ongoing!.kind })}>{t("abort")}</button>}</div>}{groups.map(([name, entries, type]) => { const selected = type === "staged" ? unstageSelection : stageSelection; const setSelected = type === "staged" ? setUnstageSelection : setStageSelection; return <ChangeGroup key={type} name={name} files={entries} type={type} selected={selected} onToggle={(path) => toggle(path, selected, setSelected)} onSelectAll={() => setSelected(entries.every((file) => selected.includes(file.path)) ? selected.filter((path) => !entries.some((file) => file.path === path)) : [...new Set([...selected, ...entries.map((file) => file.path)])])} onOpen={onOpen} onOpenExternal={onOpenExternal} onRun={onRun} />; })}</div><form className="commit-box" onSubmit={commit}><label>{t("commitMessage")}<textarea value={message} onChange={(event) => { if (repositoryId) { const value = event.target.value; setMessages((current) => ({ ...current, [repositoryId]: value })); } }} placeholder={t("commitPlaceholder")} /></label><div className="commit-options"><label><input type="checkbox" checked={amend} onChange={(event) => setAmend(event.target.checked)} /> {t("amend")}</label><label><input type="checkbox" checked={signoff} onChange={(event) => setSignoff(event.target.checked)} /> {t("signOff")}</label></div><button className="primary" disabled={commitRunning || !message.trim()}>{commitRunning ? t("running") : t("commitStaged")}</button></form></div>;
}

function RowMenu({ children, label }: { children: React.ReactNode; label?: string }) {
  const { t } = useI18n();
  const actualLabel = label ?? t("moreActions");
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const toggle = () => {
    const button = buttonRef.current; const menu = menuRef.current;
    if (!button || !menu) return;
    if (open) { menu.hidePopover(); return; }
    menu.showPopover();
    menu.style.height = "0";
    menu.style.height = `${Math.min(menu.scrollHeight + 2, window.innerHeight - 8)}px`;
    const anchor = button.getBoundingClientRect(); const bounds = menu.getBoundingClientRect();
    const top = anchor.bottom + bounds.height + 4 <= window.innerHeight ? anchor.bottom + 4 : Math.max(4, anchor.top - bounds.height - 4);
    menu.style.left = `${Math.max(4, Math.min(anchor.right - bounds.width, window.innerWidth - bounds.width - 4))}px`;
    menu.style.top = `${top}px`;
  };
  return <><button ref={buttonRef} className="row-menu-trigger" type="button" aria-label={actualLabel} aria-haspopup="menu" aria-expanded={open} onClick={toggle}>{label ? actualLabel : "•••"}</button><div ref={menuRef} className="row-menu-popover" popover="auto" role="menu" onToggle={(event) => setOpen(event.newState === "open")} onClick={(event) => { if ((event.target as HTMLElement).closest("button")) menuRef.current?.hidePopover(); }}>{children}</div></>;
}

function ChangeGroup({ name, files, type, selected, onToggle, onSelectAll, onOpen, onOpenExternal, onRun }: { name: string; files: FileChange[]; type: string; selected: string[]; onToggle: (path: string) => void; onSelectAll: () => void; onOpen: (file: FileChange, staged: boolean) => void; onOpenExternal: (path: string) => void; onRun: RunOperation }) {
  const { t } = useI18n();
  if (!files.length) return null;
  const selectable = type === "staged" || type === "unstaged" || type === "untracked";
  return <section className="change-group"><header><span>{selectable && <input type="checkbox" aria-label={`${t("selectAll")} ${name}`} checked={files.every((file) => selected.includes(file.path))} onChange={onSelectAll} />}{name}</span><code>{files.length}</code></header>{files.map((file) => <div className={`file-row ${selectable ? "selectable" : ""} ${type === "conflict" ? "conflict-row" : ""}`} key={`${type}-${file.path}`}>{selectable && <input type="checkbox" aria-label={`${type === "staged" ? t("selectFileForUnstage") : t("selectFileForStage")} ${file.path}`} checked={selected.includes(file.path)} onChange={() => onToggle(file.path)} />}<button className="file-main" onClick={() => onOpen(file, type === "staged")}><b>{file.path.split("/").at(-1)}</b><small>{file.path.includes("/") ? file.path.slice(0, file.path.lastIndexOf("/")) : "./"}</small></button><span className={`file-kind kind-${file.kind.toLowerCase()}`}>{file.kind[0]}</span>{type === "staged" ? <button onClick={() => onRun({ type: "unstageFiles", paths: [file.path] })}>{t("unstage")}</button> : type === "untracked" ? <><button onClick={() => onRun({ type: "stageFiles", paths: [file.path] })}>{t("stage")}</button><button className="danger-icon" aria-label={`${t("trash")} ${file.path}`} onClick={() => onRun({ type: "trashUntracked", paths: [file.path] })}>⌫</button></> : type === "conflict" ? <RowMenu label={t("resolve")}><button onClick={() => onOpen(file, false)}>{t("openInternalEditor")}</button><button onClick={() => onRun({ type: "chooseConflictSide", path: file.path, side: "ours" })}>{t("useCurrent")}</button><button onClick={() => onRun({ type: "chooseConflictSide", path: file.path, side: "theirs" })}>{t("useIncoming")}</button><button onClick={() => onOpenExternal(file.path)}>{t("openExternal")}</button><button onClick={() => onRun({ type: "runMergetool", path: file.path })}>{t("runMergetool")}</button><button onClick={() => onRun({ type: "markResolved", paths: [file.path] })}>{t("markResolved")}</button></RowMenu> : type === "ignored" ? null : <><button onClick={() => onRun({ type: "stageFiles", paths: [file.path] })}>{t("stage")}</button><button className="danger-icon" aria-label={`${t("discard")} ${file.path}`} onClick={() => onRun({ type: "discardTracked", paths: [file.path] })}>↶</button></>}</div>)}</section>;
}

function useVirtualRows(count: number, rowHeight: number, overscan = 12) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [range, setRange] = useState({ start: 0, end: Math.min(count, 40) });
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    let frame: number | undefined;
    const update = () => {
      frame = undefined;
      const height = container.clientHeight || 680;
      const start = Math.max(0, Math.floor(container.scrollTop / rowHeight) - overscan);
      const end = Math.min(count, Math.ceil((container.scrollTop + height) / rowHeight) + overscan);
      setRange((current) => current.start === start && current.end === end ? current : { start, end });
    };
    const scheduleUpdate = () => { if (frame === undefined) frame = window.requestAnimationFrame(update); };
    update();
    container.addEventListener("scroll", scheduleUpdate, { passive: true });
    window.addEventListener("resize", scheduleUpdate);
    return () => { container.removeEventListener("scroll", scheduleUpdate); window.removeEventListener("resize", scheduleUpdate); if (frame !== undefined) window.cancelAnimationFrame(frame); };
  }, [count, rowHeight, overscan]);
  return { containerRef, ...range, totalHeight: count * rowHeight };
}

function HistoryCanvas({ commits, selectedOid, onSelect }: { commits: CommitInfo[]; selectedOid?: string; onSelect: (oid: string) => void }) {
  const { t } = useI18n();
  const rowHeight = 34; const laneGap = 14;
  const { containerRef, start, end, totalHeight } = useVirtualRows(commits.length, rowHeight);
  const commitRows = useMemo(() => new Map(commits.map((commit, index) => [commit.oid, index])), [commits]);
  const graphWidth = useMemo(() => Math.max(44, 24 + commits.reduce((maximum, commit) => Math.max(maximum, commit.lane.column, ...commit.lane.parentColumns), 0) * laneGap), [commits]);
  const laneX = (column: number) => 12 + column * laneGap;
  const edgeBuckets = useMemo(() => {
    const buckets = new Map<number, Array<{ key: string; row: number; targetRow: number; column: number; targetColumn: number }>>();
    commits.forEach((commit, row) => commit.parents.forEach((parent, parentIndex) => {
      const targetRow = commitRows.get(parent) ?? commits.length;
      const targetColumn = targetRow === commits.length ? commit.lane.parentColumns[parentIndex] ?? commit.lane.column : commits[targetRow].lane.column;
      const edge = { key: `${commit.oid}-${parent}`, row, targetRow, column: commit.lane.column, targetColumn };
      for (let bucket = Math.floor(row / GRAPH_EDGE_BUCKET_ROWS); bucket <= Math.floor(targetRow / GRAPH_EDGE_BUCKET_ROWS); bucket += 1) {
        const entries = buckets.get(bucket);
        if (entries) entries.push(edge); else buckets.set(bucket, [edge]);
      }
    }));
    return buckets;
  }, [commits, commitRows]);
  const visibleEdges = useMemo(() => {
    const edges = new Map<string, NonNullable<ReturnType<typeof edgeBuckets.get>>[number]>();
    const first = Math.floor(start / GRAPH_EDGE_BUCKET_ROWS);
    const last = Math.floor(Math.max(start, end - 1) / GRAPH_EDGE_BUCKET_ROWS);
    for (let bucket = first; bucket <= last; bucket += 1) {
      for (const edge of edgeBuckets.get(bucket) ?? []) if (edge.row < end && edge.targetRow >= start) edges.set(edge.key, edge);
    }
    return [...edges.values()];
  }, [edgeBuckets, start, end]);
  return <div className="history-canvas"><header className="canvas-header"><strong>{t("repositoryGraph")}</strong><span>{commits.length} {t("commitsLoaded")}</span></header><div ref={containerRef} className="graph-list" style={{ "--graph-width": `${graphWidth}px` } as React.CSSProperties}><div className="virtual-history" style={{ height: totalHeight }}><svg className="commit-graph" width={graphWidth} height={totalHeight} aria-label={t("repositoryGraph")}>
    {visibleEdges.map((edge) => { const startX = laneX(edge.column); const startY = edge.row * rowHeight + rowHeight / 2; const endX = laneX(edge.targetColumn); const endY = edge.targetRow * rowHeight + rowHeight / 2; return <path className={`graph-edge lane-${edge.targetColumn % 5}`} key={edge.key} d={`M ${startX} ${startY} C ${startX} ${startY + 12}, ${endX} ${Math.max(startY + 12, endY - 12)}, ${endX} ${endY}`} />; })}
    {commits.slice(start, end).map((commit, index) => { const row = start + index; return <circle className={`graph-node lane-${commit.lane.column % 5}`} key={commit.oid} cx={laneX(commit.lane.column)} cy={row * rowHeight + rowHeight / 2} r="4" />; })}
  </svg>{commits.slice(start, end).map((commit, index) => <button style={{ position: "absolute", top: (start + index) * rowHeight }} className={`graph-row ${selectedOid === commit.oid ? "selected" : ""}`} key={commit.oid} onClick={() => onSelect(commit.oid)}><span /><code>{shortOid(commit.oid)}</code><div className="graph-subject"><strong>{commit.subject}</strong>{commit.refs.map((reference) => <span className={`ref-label ${reference.startsWith("tag: ") ? "tag" : ""}`} key={reference}>{reference}</span>)}</div><span>{commit.author}</span><time>{commit.authoredAt.slice(0, 10)}</time></button>)}</div></div></div>;
}

function HistoryPane({ commits, selectedOid, loading, hasMore, onLoadMore, onSelect, onRun }: { commits: CommitInfo[]; selectedOid?: string; loading: boolean; hasMore: boolean; onLoadMore: () => void; onSelect: (oid: string) => void; onRun: RunOperation }) {
  const { t } = useI18n();
  const rowHeight = 45;
  const { containerRef, start, end, totalHeight } = useVirtualRows(commits.length, rowHeight);
  const loadMoreRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const target = loadMoreRef.current;
    if (!target || !hasMore || !("IntersectionObserver" in window)) return;
    const observer = new IntersectionObserver((entries) => { if (entries.some((entry) => entry.isIntersecting)) onLoadMore(); }, { root: containerRef.current, rootMargin: "180px" });
    observer.observe(target);
    return () => observer.disconnect();
  }, [hasMore, onLoadMore, containerRef]);
  return <div className="history-pane"><div className="pane-title"><span>{t("commits")}</span><code>{commits.length}</code></div><div ref={containerRef} className="object-list"><div className="virtual-history" style={{ height: totalHeight + (hasMore ? 45 : 0) }}>{commits.slice(start, end).map((commit, index) => <div style={{ position: "absolute", top: (start + index) * rowHeight, width: "100%", height: rowHeight }} className={`object-action-row ${selectedOid === commit.oid ? "selected" : ""}`} key={commit.oid}><button onClick={() => onSelect(commit.oid)}><strong>{commit.subject}</strong><span>{commit.author} · {shortOid(commit.oid)}</span></button><RowMenu><button onClick={() => onRun({ type: "cherryPick", commits: [commit.oid] })}>{t("cherryPick")}</button>{commit.parents.length === 1 && <button onClick={() => onRun({ type: "revert", oid: commit.oid })}>{t("revert")}</button>}</RowMenu></div>)}{hasMore && <button ref={loadMoreRef} style={{ position: "absolute", top: totalHeight }} className="load-more" disabled={loading} onClick={onLoadMore}>{t("loadMore")}</button>}</div></div></div>;
}

function BranchCanvas({ repository }: { repository?: RepositorySummary }) { const { t } = useI18n(); return <div className="canvas-empty"><div className="branch-hero"><RailMark /><code>{repository?.branch ?? t("detachedHead")}</code></div><h2>{t("refsIntegration")}</h2><p>{t("branchHint")}</p></div>; }
function StashCanvas({ repository }: { repository?: RepositorySummary }) { const { t } = useI18n(); return <div className="canvas-empty"><div className="change-tally"><strong>≋</strong><span>{t("savedStates")}</span></div><h2>{repository?.name} {t("stashes")}</h2><p>{t("stashHint")}</p></div>; }

function BranchesPane({ repositoryId, onRun, onDialog, onDiff, onError }: { repositoryId: number; onRun: RunOperation; onDialog: (spec: DialogSpec) => void; onDiff: (diff: string) => void; onError: (message: string) => void }) {
  const { t } = useI18n();
  const [section, setSection] = useState<"branches" | "tags" | "remotes" | "submodules">("branches");
  const [creatingBranch, setCreatingBranch] = useState(false); const [branchName, setBranchName] = useState("");
  const [branches, setBranches] = useState<BranchInfo[]>([]); const [tags, setTags] = useState<TagInfo[]>([]); const [remotes, setRemotes] = useState<RemoteInfo[]>([]); const [submodules, setSubmodules] = useState<SubmoduleInfo[]>([]);
  useEffect(() => { Promise.all([api.branches(repositoryId), api.tags(repositoryId), api.remotes(repositoryId), api.submodules(repositoryId)]).then(([b, t, r, s]) => { setBranches(b); setTags(t); setRemotes(r); setSubmodules(s); }).catch((error) => onError(errorMessage(error))); }, [repositoryId, onError]);
  const createBranch = (event: React.FormEvent) => { event.preventDefault(); const name = branchName.trim(); if (!name) return; onRun({ type: "createBranch", name, checkout: true }); setBranchName(""); setCreatingBranch(false); };
  const compare = () => onDialog({ title: t("compare"), fields: [{ name: "base", label: t("baseBranch"), required: true }, { name: "head", label: t("headBranch"), required: true }], onSubmit: ({ base, head }) => api.compareBranches(repositoryId, String(base).trim(), String(head).trim()).then(onDiff).catch((error) => onError(errorMessage(error))) });
  const renameBranch = (oldName: string) => onDialog({ title: t("rename"), fields: [{ name: "name", label: t("newBranchName"), value: oldName, required: true }], onSubmit: ({ name }) => onRun({ type: "renameBranch", oldName, newName: String(name).trim() }) });
  const createTag = () => onDialog({ title: t("create"), fields: [{ name: "name", label: t("tagName"), required: true }, { name: "message", label: t("annotation") }], onSubmit: ({ name, message }) => onRun({ type: "createTag", name: String(name).trim(), message: String(message).trim() || undefined }) });
  const pushTag = (name: string) => onDialog({ title: t("pushTag"), fields: [{ name: "remote", label: t("remote"), value: "origin", required: true }], onSubmit: ({ remote }) => onRun({ type: "pushTag", remote: String(remote).trim(), name }) });
  const addRemote = () => onDialog({ title: t("add"), fields: [{ name: "name", label: t("remoteName"), required: true }, { name: "url", label: t("remoteUrl"), required: true }], onSubmit: ({ name, url }) => onRun({ type: "addRemote", name: String(name).trim(), url: String(url).trim() }) });
  const editRemote = (name: string, value: string) => onDialog({ title: t("editUrl"), fields: [{ name: "url", label: t("newRemoteUrl"), value, required: true }], onSubmit: ({ url }) => onRun({ type: "setRemoteUrl", name, url: String(url).trim() }) });
  const updateSubmodules = () => onDialog({ title: t("update"), fields: [{ name: "recursive", label: t("updateNested"), value: false, type: "checkbox" }], onSubmit: ({ recursive }) => onRun({ type: "submoduleUpdate", paths: [], recursive: Boolean(recursive) }) });
  const branchGroups = [[t("localBranches"), branches.filter((branch) => !branch.remote)], [t("remoteBranches"), branches.filter((branch) => branch.remote)]] as const;
  return <div><div className="segmented">{(["branches", "tags", "remotes", "submodules"] as const).map((item) => <button className={section === item ? "active" : ""} key={item} onClick={() => setSection(item)}>{t(item)}</button>)}</div>{section === "branches" && <><div className="pane-title"><span>{t("branches")}</span><span><button onClick={compare}>{t("compare")}</button><button onClick={() => setCreatingBranch(true)}>{t("newBranch")}</button></span></div>{creatingBranch && <form className="new-branch-form" onSubmit={createBranch}><input autoFocus aria-label={t("newBranchName")} value={branchName} onChange={(event) => setBranchName(event.target.value)} onKeyDown={(event) => { if (event.key === "Escape") { setBranchName(""); setCreatingBranch(false); } }} /><button type="submit" disabled={!branchName.trim()}>{t("create")}</button><button type="button" onClick={() => { setBranchName(""); setCreatingBranch(false); }}>{t("cancel")}</button></form>}<div className="object-list">{branchGroups.map(([name, entries]) => entries.length > 0 && <section className="branch-group" key={name}><header><span>{name}</span><code>{entries.length}</code></header>{entries.map((branch) => <div className="object-action-row" key={`${branch.remote}-${branch.name}`}><button className={branch.current ? "current" : ""} onDoubleClick={() => !branch.remote && !branch.current && onRun({ type: "switchBranch", name: branch.name })}><strong>{branch.current && "● "}{branch.name}</strong><span>{shortOid(branch.oid)} {branch.upstream && `· ${branch.upstream}`}</span></button><RowMenu>{branch.remote ? <button onClick={() => { const [remote, ...parts] = branch.name.split("/"); onRun({ type: "deleteRemoteBranch", remote, branch: parts.join("/") }); }}>{t("deleteRemoteBranch")}</button> : <>{!branch.current && <button onClick={() => onRun({ type: "switchBranch", name: branch.name })}>{t("switch")}</button>}{!branch.current && <button onClick={() => onRun({ type: "merge", reference: branch.name, mode: "normal" })}>{t("merge")}</button>}{!branch.current && <button onClick={() => onRun({ type: "merge", reference: branch.name, mode: "fastForward" })}>{t("fastForward")}</button>}{!branch.current && <button onClick={() => onRun({ type: "merge", reference: branch.name, mode: "squash" })}>{t("squashMerge")}</button>}{!branch.current && <button onClick={() => onRun({ type: "rebase", onto: branch.name })}>{t("rebaseOnto")}</button>}<button onClick={() => renameBranch(branch.name)}>{t("rename")}</button>{!branch.current && <button onClick={() => onRun({ type: "deleteBranch", name: branch.name, force: false })}>{t("delete")}</button>}{!branch.current && <button onClick={() => onRun({ type: "deleteBranch", name: branch.name, force: true })}>{t("forceDelete")}</button>}</>}</RowMenu></div>)}</section>)}</div></>}{section === "tags" && <><div className="pane-title"><span>{t("tags")}</span><button onClick={createTag}>＋</button></div><div className="object-list">{tags.map((tag) => <div className="object-action-row" key={tag.name}><button><strong>{tag.name}</strong><span>{tag.subject || shortOid(tag.oid)}</span></button><RowMenu><button onClick={() => pushTag(tag.name)}>{t("pushTag")}</button><button onClick={() => onRun({ type: "deleteLocalTag", name: tag.name })}>{t("deleteLocalTag")}</button></RowMenu></div>)}</div></>}{section === "remotes" && <><div className="pane-title"><span>{t("remotes")}</span><button onClick={addRemote}>＋</button></div><div className="object-list">{remotes.map((remote) => <div className="object-action-row" key={remote.name}><button><strong>{remote.name}</strong><span>{remote.fetchUrl}</span></button><RowMenu><button onClick={() => editRemote(remote.name, remote.fetchUrl)}>{t("editUrl")}</button><button onClick={() => onRun({ type: "removeRemote", name: remote.name })}>{t("removeRemote")}</button></RowMenu></div>)}</div></>}{section === "submodules" && <><div className="pane-title"><span>{t("submodules")}</span><span><button onClick={() => onRun({ type: "submoduleInit", paths: [], recursive: false })}>{t("init")}</button><button onClick={() => onRun({ type: "submoduleSync", paths: [], recursive: false })}>{t("sync")}</button><button onClick={updateSubmodules}>{t("update")}</button></span></div><div className="object-list">{submodules.map((module) => <button key={module.path}><strong>{module.path}</strong><span>{module.state} · {shortOid(module.oid)}</span></button>)}</div></>}</div>;
}

function StashesPane({ repositoryId, onRun, onDialog, onError }: { repositoryId: number; onRun: RunOperation; onDialog: (spec: DialogSpec) => void; onError: (message: string) => void }) {
  const { t } = useI18n();
  const [stashes, setStashes] = useState<StashInfo[]>([]);
  useEffect(() => { api.stashes(repositoryId).then(setStashes).catch((error) => onError(errorMessage(error))); }, [repositoryId, onError]);
  const create = () => onDialog({ title: t("create"), fields: [{ name: "message", label: t("stashMessage") }, { name: "includeUntracked", label: t("includeUntracked"), value: false, type: "checkbox" }], onSubmit: ({ message, includeUntracked }) => onRun({ type: "stashCreate", message: String(message).trim() || undefined, includeUntracked: Boolean(includeUntracked) }) });
  return <div><div className="pane-title"><span>{t("stashes")}</span><button onClick={create}>＋</button></div><div className="object-list">{stashes.map((stash) => <div className="stash-row" key={stash.oid}><button><strong>stash@{`{${stash.index}}`}</strong><span>{stash.subject}</span></button><div><button onClick={() => onRun({ type: "stashApply", index: stash.index, pop: false })}>{t("apply")}</button><button onClick={() => onRun({ type: "stashApply", index: stash.index, pop: true })}>{t("pop")}</button><button onClick={() => onRun({ type: "stashDrop", index: stash.index })}>{t("drop")}</button></div></div>)}</div></div>;
}

function CommandPalette({ items, onClose }: { items: CommandItem[]; onClose: () => void }) {
  const { t } = useI18n();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const visible = items.filter((item) => item.search.includes(query.trim().toLowerCase()));
  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (typeof dialog.showModal === "function") dialog.showModal(); else dialog.setAttribute("open", "");
    return () => { if (dialog.open && typeof dialog.close === "function") dialog.close(); };
  }, []);
  useEffect(() => { setActive(0); }, [query]);
  const run = (item?: CommandItem) => { if (item) { onClose(); item.action(); } };
  return <dialog ref={dialogRef} className="command-palette" aria-labelledby="command-palette-title" onCancel={(event) => { event.preventDefault(); onClose(); }} onClose={onClose}><header id="command-palette-title">{t("commandPalette")}<kbd>⌘K</kbd></header><input autoFocus role="combobox" aria-controls="command-list" aria-expanded="true" aria-activedescendant={visible[active] ? `command-${visible[active].id}` : undefined} placeholder={t("searchCommands")} value={query} onChange={(event) => setQuery(event.target.value)} onKeyDown={(event) => {
    if (event.key === "ArrowDown") { event.preventDefault(); setActive((index) => visible.length ? (index + 1) % visible.length : 0); }
    if (event.key === "ArrowUp") { event.preventDefault(); setActive((index) => visible.length ? (index - 1 + visible.length) % visible.length : 0); }
    if (event.key === "Enter") { event.preventDefault(); run(visible[active]); }
  }} /><div id="command-list" role="listbox">{visible.map((item, index) => <button id={`command-${item.id}`} role="option" aria-selected={index === active} className={index === active ? "active" : ""} key={item.id} onMouseEnter={() => setActive(index)} onClick={() => run(item)}>{item.label}</button>)}</div></dialog>;
}

function FormDialog({ spec, onClose }: { spec: DialogSpec; onClose: () => void }) {
  const { t } = useI18n();
  const [values, setValues] = useState<Record<string, DialogValue>>(() => Object.fromEntries((spec.fields ?? []).map((field) => [field.name, field.value ?? (field.type === "checkbox" ? false : "")])));
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState("");
  const valid = (spec.fields ?? []).every((field) => !field.required || String(values[field.name] ?? "").trim());
  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!valid || submitting) return;
    setSubmitting(true); setError("");
    try { await spec.onSubmit(values); onClose(); }
    catch (cause) { setError(errorMessage(cause)); setSubmitting(false); }
  };
  return <div className="modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}><form className="form-dialog" role="dialog" aria-modal="true" aria-labelledby="form-dialog-title" onSubmit={submit} onKeyDown={(event) => { if (event.key === "Escape") onClose(); }}><header><h2 id="form-dialog-title">{spec.title}</h2></header>{spec.message && <p>{spec.message}</p>}{(spec.fields ?? []).map((field, index) => field.type === "checkbox" ? <label className="dialog-checkbox" key={field.name}><input type="checkbox" checked={Boolean(values[field.name])} onChange={(event) => setValues((current) => ({ ...current, [field.name]: event.target.checked }))} /><span>{field.label}</span></label> : <label className="dialog-field" key={field.name}><span>{field.label}</span><input autoFocus={index === 0} value={String(values[field.name] ?? "")} required={field.required} onChange={(event) => setValues((current) => ({ ...current, [field.name]: event.target.value }))} /></label>)}{error && <p className="dialog-error">{error}</p>}<footer><button type="button" onClick={onClose}>{t("cancel")}</button><button className={spec.danger ? "danger" : "primary"} type="submit" disabled={!valid || submitting}>{spec.submitLabel ?? t("confirm")}</button></footer></form></div>;
}

function ConfirmDialog({ pending, onCancel, onConfirm }: { pending: Pending; onCancel: () => void; onConfirm: () => void }) {
  const { t } = useI18n();
  return <div className="modal-backdrop" role="presentation"><section className={`confirm-dialog risk-${pending.preview.risk}`} role="alertdialog" aria-modal="true" aria-labelledby="confirm-title"><div className="risk-stripe" /><header><span>{pending.preview.risk === "destructive" ? t("irreversible") : t("reviewOperation")}</span><h2 id="confirm-title">{pending.preview.title}</h2></header><p>{pending.preview.summary}</p>{pending.preview.affectedPaths.length > 0 && <div className="impact"><label>{t("affectedPaths")}</label>{pending.preview.affectedPaths.map((path) => <code key={path}>{path}</code>)}</div>}{pending.preview.affectedRefs.length > 0 && <div className="impact"><label>{t("affectedRefs")}</label>{pending.preview.affectedRefs.map((ref) => <code key={ref}>{ref}</code>)}</div>}<footer><span>{pending.preview.recoverable ? t("recoverable") : t("unrecoverable")}</span><button onClick={onCancel}>{t("cancel")}</button><button className="danger" onClick={onConfirm}>{pending.preview.title}</button></footer></section></div>;
}

const MemoRepositoryRow = memo(RepositoryRow);
const MemoChangesOverview = memo(ChangesOverview);
const MemoChangesPane = memo(ChangesPane);
const MemoDiffView = memo(DiffView);
const MemoHistoryCanvas = memo(HistoryCanvas);
const MemoHistoryPane = memo(HistoryPane);
const MemoBranchesPane = memo(BranchesPane);
const MemoStashesPane = memo(StashesPane);
