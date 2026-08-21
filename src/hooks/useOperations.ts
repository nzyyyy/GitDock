import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, type OperationEvent, type OperationRequest } from "../api";
import { translate } from "../i18n";
import { errorMessage, type DialogSpec, type OperationFinished, type OperationOutcome, type OperationToast, type Pending } from "../types";

export function useOperations({
  pushLog, reportError, t, showDialog, setSelectedId, setOutputOpen, refreshRepositories, refreshRepository, refreshHistory, selectedId, selectedIdRef, historyRepositoryRef,
}: {
  pushLog: (kind: "stdout" | "stderr" | "started" | "finished" | "error", message: string) => void;
  reportError: (message: string) => void;
  t: (key: Parameters<typeof translate>[1]) => string;
  showDialog: (spec: DialogSpec) => void;
  setSelectedId: React.Dispatch<React.SetStateAction<number | undefined>>;
  setOutputOpen: React.Dispatch<React.SetStateAction<boolean>>;
  refreshRepositories: () => Promise<void>;
  refreshRepository: (repositoryId: number) => Promise<void>;
  refreshHistory: (repositoryId: number) => Promise<void>;
  selectedId?: number;
  selectedIdRef: React.MutableRefObject<number | undefined>;
  historyRepositoryRef: React.MutableRefObject<number | undefined>;
}) {
  const [pending, setPending] = useState<Pending>();
  const [busyOperations, setBusyOperations] = useState<number[]>([]);
  const [toasts, setToasts] = useState<OperationToast[]>([]);
  const allowClose = useRef(false);
  const cloneOperations = useRef(new Set<number>());
  const operationCallbacks = useRef(new Map<number, OperationFinished>());
  const earlyCompletions = useRef(new Map<number, OperationOutcome>());
  const operationTitles = useRef(new Map<number, string>());

  const dismissToast = useCallback((id: number) => setToasts((current) => current.filter((toast) => toast.id !== id)), []);

  const startOperation = useCallback(async (repositoryId: number, request: OperationRequest, confirmed: boolean, onFinished?: OperationFinished) => {
    const result = await api.startOperation(repositoryId, request, confirmed);
    const finished = onFinished || request.type === "commit" || request.type === "fetch" ? (outcome: OperationOutcome) => {
      if (outcome === "succeeded" && (request.type === "commit" || request.type === "fetch")) void refreshRepository(repositoryId);
      if (outcome === "succeeded" && request.type === "commit" && historyRepositoryRef.current === repositoryId) {
        if (selectedIdRef.current === repositoryId) void refreshHistory(repositoryId); else historyRepositoryRef.current = undefined;
      }
      onFinished?.(outcome);
    } : undefined;
    if (finished) {
      const outcome = earlyCompletions.current.get(result.operationId);
      if (outcome) { earlyCompletions.current.delete(result.operationId); finished(outcome); }
      else operationCallbacks.current.set(result.operationId, finished);
    }
  }, [refreshHistory, refreshRepository]);

  const run = useCallback(async (request: OperationRequest, onFinished?: OperationFinished) => {
    if (!selectedId) { onFinished?.("failed"); return; }
    try {
      const preview = await api.previewOperation(selectedId, request);
      if (preview.requiresConfirmation) { setPending({ repositoryId: selectedId, request, preview, onFinished }); return; }
      await startOperation(selectedId, request, false, onFinished);
    } catch (error) { onFinished?.("failed"); reportError(errorMessage(error)); }
  }, [selectedId, reportError, startOperation]);

  const confirmPending = async () => {
    if (!pending) return;
    try { await startOperation(pending.repositoryId, pending.request, true, pending.onFinished); setPending(undefined); }
    catch (error) { pending.onFinished?.("failed"); reportError(errorMessage(error)); }
  };

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
    ]);
    return () => { unlisteners.then((values) => values.forEach((unlisten) => unlisten())); };
  }, [pushLog, refreshRepositories, setSelectedId, setOutputOpen]);

  useEffect(() => {
    const listener = getCurrentWindow().onCloseRequested(async (event) => {
      if (allowClose.current || !busyOperations.length) return;
      event.preventDefault();
      showDialog({
        title: t("confirm"), message: `${busyOperations.length} ${t("closeOperations")}`, danger: true,
        onSubmit: async () => {
          await Promise.allSettled(busyOperations.map(api.cancelOperation));
          allowClose.current = true;
          await getCurrentWindow().close();
        },
      });
    });
    return () => { listener.then((unlisten) => unlisten()); };
  }, [busyOperations, t, showDialog]);

  return { pending, setPending, confirmPending, busyOperations, toasts, dismissToast, run, startOperation };
}
