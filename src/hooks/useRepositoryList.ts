import { startTransition, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, type RepositorySummary, type WorkingTreeSnapshot } from "../api";
import { translate } from "../i18n";
import { errorMessage, FAVORITES_GROUP, UNGROUPED_GROUP, type RepositoryGroup } from "../types";

type Translate = (key: Parameters<typeof translate>[1]) => string;

export function useRepositoryList({
  reportError, selectedIdRef, setSnapshot, refreshStatus, t, language,
}: {
  reportError: (message: string) => void;
  selectedIdRef: React.MutableRefObject<number | undefined>;
  setSnapshot: React.Dispatch<React.SetStateAction<WorkingTreeSnapshot | undefined>>;
  refreshStatus: (repositoryId?: number, includeIgnored?: boolean) => Promise<WorkingTreeSnapshot | undefined>;
  t: Translate;
  language: "en" | "zh-CN";
}) {
  const [repositories, setRepositories] = useState<RepositorySummary[]>([]);
  const [customGroups, setCustomGroups] = useState<string[]>([]);
  const [filter, setFilter] = useState("");
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(() => new Set());
  const draggingRepositoryId = useRef<number | undefined>(undefined);
  const draggingHeight = useRef(0);
  const dropHintRef = useRef<{ groupKey: string; index: number } | null>(null);
  const dragSlots = useRef(new Map<string, number[]>());
  const [draggingId, setDraggingId] = useState<number>();
  const [dropHint, setDropHint] = useState<{ groupKey: string; index: number } | null>(null);
  const repositoryListRequest = useRef(0);
  const repositoryRequests = useRef(new Map<number, number>());
  const streamedSummaries = useRef(new Map<number, RepositorySummary>());

  const refreshRepositories = useCallback(async () => {
    const request = ++repositoryListRequest.current;
    streamedSummaries.current.clear();
    try {
      const summaries = await api.refreshRepositories(selectedIdRef.current ?? null);
      if (request !== repositoryListRequest.current) return;
      setRepositories(summaries.map((summary) => streamedSummaries.current.get(summary.id) ?? summary));
    }
    catch (error) { if (request === repositoryListRequest.current) { reportError(errorMessage(error)); } }
  }, [reportError]);

  const refreshRepository = useCallback(async (repositoryId: number) => {
    const request = (repositoryRequests.current.get(repositoryId) ?? 0) + 1;
    repositoryRequests.current.set(repositoryId, request);
    try {
      const refresh = await api.refreshRepository(repositoryId);
      if (repositoryRequests.current.get(repositoryId) !== request) return;
      streamedSummaries.current.set(repositoryId, refresh.summary);
      const selected = repositoryId === selectedIdRef.current;
      startTransition(() => {
        setRepositories((current) => current.map((item) => item.id === repositoryId ? refresh.summary : item));
        if (selected) {
          const refreshedSnapshot = refresh.snapshot;
          if (refreshedSnapshot) setSnapshot((current) => repositoryId === selectedIdRef.current ? refreshedSnapshot : current);
          else if (refresh.summary.kind !== "workTree") setSnapshot((current) => repositoryId === selectedIdRef.current ? undefined : current);
        }
      });
      if (selected && refresh.summary.kind === "workTree" && !refresh.snapshot) void refreshStatus(repositoryId);
    } catch (error) {
      if (repositoryRequests.current.get(repositoryId) !== request) return;
      reportError(errorMessage(error));
    }
  }, [reportError, refreshStatus]);

  useEffect(() => {
    const unlisteners = Promise.all([
      listen<{ repositoryId: number }>("repository-changed", ({ payload }) => {
        refreshRepository(payload.repositoryId);
      }),
      listen<RepositorySummary>("repository-summary-refreshed", ({ payload }) => {
        streamedSummaries.current.set(payload.id, payload);
        startTransition(() => setRepositories((current) => current.map((repository) => repository.id === payload.id ? payload : repository)));
      }),
      listen("repository-list-changed", refreshRepositories),
    ]);
    return () => { unlisteners.then((values) => values.forEach((unlisten) => unlisten())); };
  }, [refreshRepositories, refreshRepository]);

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
    const named = new Map([...grouped.entries()].filter(([key]) => key !== UNGROUPED_GROUP));
    for (const key of customGroups) {
      if (!named.has(key) && key !== FAVORITES_GROUP && key !== UNGROUPED_GROUP) named.set(key, []);
    }
    const namedGroups = [...named.entries()]
      .sort(([left], [right]) => {
        const leftIndex = customGroups.indexOf(left);
        const rightIndex = customGroups.indexOf(right);
        if (leftIndex !== -1 || rightIndex !== -1) {
          if (leftIndex === -1) return 1;
          if (rightIndex === -1) return -1;
          return leftIndex - rightIndex;
        }
        return left.localeCompare(right);
      })
      .map(([key, items]) => ({ key, label: key, repositories: items }));
    return [
      { key: FAVORITES_GROUP, label: t("favorites"), repositories: favorites },
      ...namedGroups,
      { key: UNGROUPED_GROUP, label: t("ungrouped"), repositories: grouped.get(UNGROUPED_GROUP) ?? [] },
    ];
  }, [repositories, filter, language, customGroups]);

  const persistRepositoryLayout = useCallback(async (ordered: RepositorySummary[]) => {
    const previous = repositories;
    const next = ordered.map((repository, order) => ({ ...repository, order }));
    setRepositories(next);
    try {
      await api.reorderRepositories(next.map(({ id, group, favorite, order }) => ({ id, group, favorite, order })));
    } catch (error) {
      setRepositories(previous); reportError(errorMessage(error));
    }
  }, [repositories, reportError]);

  const moveRepository = useCallback((repositoryId: number, targetGroup: string, targetId?: number, after = false) => {
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
      : { ...moving, favorite: false, group: targetGroup === UNGROUPED_GROUP ? null : targetGroup };
    const index = targetId ? target.repositories.findIndex((repository) => repository.id === targetId) : -1;
    target.repositories.splice(index < 0 ? target.repositories.length : index + Number(after), 0, moving);
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

  const persistCustomGroups = useCallback((next: string[]) => {
    const previous = customGroups;
    setCustomGroups(next);
    api.saveGroupOrder(next).catch((error) => { setCustomGroups(previous); reportError(errorMessage(error)); });
  }, [customGroups, reportError]);

  const addGroup = useCallback((name: string) => {
    const key = name.trim();
    if (!key || key === FAVORITES_GROUP || key === UNGROUPED_GROUP) return;
    if (customGroups.includes(key) || repositoryGroups.some((group) => group.key === key)) return;
    persistCustomGroups([...customGroups, key]);
  }, [customGroups, repositoryGroups, persistCustomGroups]);

  const updateGroup = useCallback((group: string, replacement?: string) => {
    const ordered = [...repositories].sort((left, right) => left.order - right.order).map((repository) => repository.group === group ? { ...repository, group: replacement ?? null } : repository);
    void persistRepositoryLayout(ordered);
    const next = replacement === undefined
      ? customGroups.filter((key) => key !== group)
      : customGroups.map((key) => key === group ? replacement : key);
    if (next.length !== customGroups.length || next.some((key, index) => key !== customGroups[index])) {
      persistCustomGroups(next);
    }
  }, [repositories, persistRepositoryLayout, customGroups, persistCustomGroups]);

  const acceptRepositoryDrop = useCallback((event: React.DragEvent) => {
    if (filter.trim()) return;
    event.preventDefault(); event.dataTransfer.dropEffect = "move";
  }, [filter]);

  const setHint = (hint: { groupKey: string; index: number } | null) => {
    dropHintRef.current = hint;
    setDropHint((current) => current?.groupKey === hint?.groupKey && current?.index === hint?.index ? current : hint);
  };

  const clearRepositoryDropHint = useCallback(() => setHint(null), []);

  const captureGroupSlots = (groupEl: HTMLElement, groupKey: string) => {
    if (dragSlots.current.has(groupKey)) return;
    const dragging = draggingRepositoryId.current;
    dragSlots.current.set(
      groupKey,
      [...groupEl.querySelectorAll<HTMLElement>(".repo-row-shell")]
        .filter((row) => Number(row.dataset.repositoryId) !== dragging)
        .map((row) => {
          const rect = row.getBoundingClientRect();
          return rect.top + rect.height / 2;
        }),
    );
  };

  const beginRepositoryDrag = useCallback((repositoryId: number, height: number, groupKey: string, groupEl?: HTMLElement | null) => {
    if (filter.trim()) return;
    const group = repositoryGroups.find((item) => item.key === groupKey);
    const index = group?.repositories.findIndex((repository) => repository.id === repositoryId) ?? 0;
    draggingRepositoryId.current = repositoryId;
    draggingHeight.current = height;
    dragSlots.current.clear();
    if (groupEl) captureGroupSlots(groupEl, groupKey);
    setDraggingId(repositoryId);
    setHint({ groupKey, index });
  }, [filter, repositoryGroups]);

  const endRepositoryDrag = useCallback(() => {
    draggingRepositoryId.current = undefined;
    draggingHeight.current = 0;
    dragSlots.current.clear();
    setDraggingId(undefined);
    setHint(null);
  }, []);

  const hintRepositoryDrop = useCallback((event: React.DragEvent<HTMLElement>, groupKey: string) => {
    if (filter.trim() || draggingRepositoryId.current === undefined) return;
    event.preventDefault(); event.dataTransfer.dropEffect = "move";
    captureGroupSlots(event.currentTarget, groupKey);
    const slots = dragSlots.current.get(groupKey) ?? [];
    const found = slots.findIndex((mid) => event.clientY < mid);
    setHint({ groupKey, index: found < 0 ? slots.length : found });
  }, [filter]);

  const dropRepository = useCallback((groupKey: string) => {
    const repositoryId = draggingRepositoryId.current;
    if (filter.trim() || !repositoryId) return;
    const hint = dropHintRef.current?.groupKey === groupKey ? dropHintRef.current : undefined;
    const remaining = (repositoryGroups.find((group) => group.key === groupKey)?.repositories ?? [])
      .filter((repository) => repository.id !== repositoryId);
    const targetId = hint ? remaining[hint.index]?.id : undefined;
    moveRepository(repositoryId, groupKey, targetId, false);
  }, [filter, moveRepository, repositoryGroups]);

  const repositoryRowShift = useCallback((groupKey: string, repositoryId: number) => {
    if (!dropHint || !draggingId || repositoryId === draggingId) return 0;
    const group = repositoryGroups.find((item) => item.key === groupKey);
    if (!group) return 0;
    const height = draggingHeight.current;
    const oldIndex = group.repositories.findIndex((repository) => repository.id === repositoryId);
    if (oldIndex < 0) return 0;
    const dragIndex = group.repositories.findIndex((repository) => repository.id === draggingId);
    if (dropHint.groupKey !== groupKey) return dragIndex >= 0 && oldIndex > dragIndex ? -height : 0;
    if (dragIndex < 0) return oldIndex >= dropHint.index ? height : 0;
    let next = oldIndex > dragIndex ? oldIndex - 1 : oldIndex;
    if (next >= dropHint.index) next += 1;
    return (next - oldIndex) * height;
  }, [dropHint, draggingId, repositoryGroups]);

  return {
    repositories, setRepositories, customGroups, setCustomGroups, filter, setFilter, collapsedGroups, setCollapsedGroups,
    draggingId, draggingHeight: draggingId ? draggingHeight.current : 0, dropHint, refreshRepositories, refreshRepository,
    repositoryGroups, moveRepository, moveRepositoryBy, addGroup, updateGroup,
    acceptRepositoryDrop, hintRepositoryDrop, clearRepositoryDropHint, beginRepositoryDrag, endRepositoryDrag, dropRepository, repositoryRowShift,
  };
}
