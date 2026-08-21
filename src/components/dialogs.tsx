import { useEffect, useRef, useState } from "react";
import { useI18n } from "../i18n";
import { errorMessage, type DialogSpec, type DialogValue, type Pending } from "../types";

export function FormDialog({ spec, onClose }: { spec: DialogSpec; onClose: () => void }) {
  const { t } = useI18n();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [values, setValues] = useState<Record<string, DialogValue>>(() => Object.fromEntries((spec.fields ?? []).map((field) => [field.name, field.value ?? (field.type === "checkbox" ? false : "")])));
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState("");
  const errorRef = useRef<HTMLParagraphElement>(null);
  const valid = (spec.fields ?? []).every((field) => !field.required || String(values[field.name] ?? "").trim());
  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!valid || submitting) return;
    setSubmitting(true); setError("");
    try { await spec.onSubmit(values); onClose(); }
    catch (cause) { setError(errorMessage(cause)); setSubmitting(false); }
  };
  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (typeof dialog.showModal === "function") dialog.showModal(); else dialog.setAttribute("open", "");
    return () => { if (dialog.open && typeof dialog.close === "function") dialog.close(); };
  }, []);
  useEffect(() => { if (error) errorRef.current?.focus(); }, [error]);
  return <dialog ref={dialogRef} className="form-dialog" aria-labelledby="form-dialog-title" onCancel={(event) => { event.preventDefault(); onClose(); }} onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}><form className="dialog-contents" onSubmit={submit}><header><h2 id="form-dialog-title">{spec.title}</h2></header>{spec.message && <p>{spec.message}</p>}{(spec.fields ?? []).map((field, index) => field.type === "checkbox" ? <label className="dialog-checkbox" key={field.name}><input name={field.name} type="checkbox" autoComplete="off" checked={Boolean(values[field.name])} onChange={(event) => setValues((current) => ({ ...current, [field.name]: event.target.checked }))} /><span>{field.label}</span></label> : <label className="dialog-field" key={field.name}><span>{field.label}</span><input autoFocus={index === 0} name={field.name} type={field.type ?? "text"} autoComplete="off" value={String(values[field.name] ?? "")} required={field.required} onChange={(event) => setValues((current) => ({ ...current, [field.name]: event.target.value }))} /></label>)}{error && <p ref={errorRef} className="dialog-error" role="alert" tabIndex={-1}>{error}</p>}<footer><button type="button" onClick={onClose}>{t("cancel")}</button><button className={spec.danger ? "danger" : "primary"} type="submit" disabled={!valid || submitting}>{submitting ? t("loading") : spec.submitLabel ?? t("confirm")}</button></footer></form></dialog>;
}

export function ConfirmDialog({ pending, onCancel, onConfirm }: { pending: Pending; onCancel: () => void; onConfirm: () => void }) {
  const { t } = useI18n();
  const dialogRef = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (typeof dialog.showModal === "function") dialog.showModal(); else dialog.setAttribute("open", "");
    return () => { if (dialog.open && typeof dialog.close === "function") dialog.close(); };
  }, []);
  return <dialog ref={dialogRef} className={`confirm-dialog risk-${pending.preview.risk}`} role="alertdialog" aria-labelledby="confirm-title" onCancel={(event) => { event.preventDefault(); onCancel(); }}><div className="risk-stripe" aria-hidden="true" /><header><span>{pending.preview.risk === "destructive" ? t("irreversible") : t("reviewOperation")}</span><h2 id="confirm-title">{pending.preview.title}</h2></header><p>{pending.preview.summary}</p>{pending.preview.affectedPaths.length > 0 && <div className="impact"><span>{t("affectedPaths")}</span>{pending.preview.affectedPaths.map((path) => <code key={path}>{path}</code>)}</div>}{pending.preview.affectedRefs.length > 0 && <div className="impact"><span>{t("affectedRefs")}</span>{pending.preview.affectedRefs.map((ref) => <code key={ref}>{ref}</code>)}</div>}<footer><span>{pending.preview.recoverable ? t("recoverable") : t("unrecoverable")}</span><button autoFocus onClick={onCancel}>{t("cancel")}</button><button className="danger" onClick={onConfirm}>{pending.preview.title}</button></footer></dialog>;
}
