import { useState } from "react";
import { useI18n } from "../i18n";
import { errorMessage, type DialogSpec, type DialogValue, type Pending } from "../types";

export function FormDialog({ spec, onClose }: { spec: DialogSpec; onClose: () => void }) {
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

export function ConfirmDialog({ pending, onCancel, onConfirm }: { pending: Pending; onCancel: () => void; onConfirm: () => void }) {
  const { t } = useI18n();
  return <div className="modal-backdrop" role="presentation"><section className={`confirm-dialog risk-${pending.preview.risk}`} role="alertdialog" aria-modal="true" aria-labelledby="confirm-title"><div className="risk-stripe" /><header><span>{pending.preview.risk === "destructive" ? t("irreversible") : t("reviewOperation")}</span><h2 id="confirm-title">{pending.preview.title}</h2></header><p>{pending.preview.summary}</p>{pending.preview.affectedPaths.length > 0 && <div className="impact"><label>{t("affectedPaths")}</label>{pending.preview.affectedPaths.map((path) => <code key={path}>{path}</code>)}</div>}{pending.preview.affectedRefs.length > 0 && <div className="impact"><label>{t("affectedRefs")}</label>{pending.preview.affectedRefs.map((ref) => <code key={ref}>{ref}</code>)}</div>}<footer><span>{pending.preview.recoverable ? t("recoverable") : t("unrecoverable")}</span><button onClick={onCancel}>{t("cancel")}</button><button className="danger" onClick={onConfirm}>{pending.preview.title}</button></footer></section></div>;
}
