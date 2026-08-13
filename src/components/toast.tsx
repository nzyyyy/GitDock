import { useEffect } from "react";
import { useI18n } from "../i18n";
import type { OperationToast } from "../types";

export function ToastStack({ toasts, onDismiss }: { toasts: OperationToast[]; onDismiss: (id: number) => void }) {
  return <div className="toast-stack">{toasts.map((toast) => <OperationToastView key={toast.id} toast={toast} onDismiss={onDismiss} />)}</div>;
}

function OperationToastView({ toast, onDismiss }: { toast: OperationToast; onDismiss: (id: number) => void }) {
  const { t } = useI18n();
  useEffect(() => { const timer = window.setTimeout(() => onDismiss(toast.id), 3_000); return () => window.clearTimeout(timer); }, [toast.id, onDismiss]);
  const result = toast.outcome === "succeeded" ? t("operationSucceeded") : toast.outcome === "cancelled" ? t("operationCancelled") : t("operationFailed");
  return <div className={`operation-toast toast-${toast.outcome}`} role={toast.outcome === "failed" ? "alert" : "status"}><span><strong>{toast.title}</strong><small>{result}{toast.outcome === "failed" && toast.message ? ` · ${toast.message}` : ""}</small></span><button aria-label={t("dismissNotification")} onClick={() => onDismiss(toast.id)}>×</button></div>;
}
