import type { OperationEvent, OperationPreview, OperationRequest, RepositorySummary } from "./api";

export type Tab = "changes" | "history" | "branches" | "stashes";
export type RepositoryGroup = { key: string; label: string; repositories: RepositorySummary[] };
export type OperationOutcome = NonNullable<OperationEvent["outcome"]>;
export type OperationToast = { id: number; title: string; message: string; outcome: OperationOutcome };
export type OperationFinished = (outcome: OperationOutcome) => void;
export type RunOperation = (request: OperationRequest, onFinished?: OperationFinished) => void | Promise<void>;
export type Pending = { repositoryId: number; request: OperationRequest; preview: OperationPreview; onFinished?: OperationFinished };
export type DialogValue = string | boolean;
export type DialogField = { name: string; label: string; value?: DialogValue; required?: boolean; type?: "text" | "checkbox" };
export type DialogSpec = { title: string; message?: string; submitLabel?: string; danger?: boolean; fields?: DialogField[]; onSubmit: (values: Record<string, DialogValue>) => void | Promise<void> };
export type CommandItem = { id: string; label: string; search: string; action: () => void };

export const shortOid = (oid?: string | null) => oid?.slice(0, 8) ?? "—";
export const errorMessage = (error: unknown) => error instanceof Error ? error.message : String(error);
export const FAVORITES_GROUP = "\0favorites";
export const UNGROUPED_GROUP = "\0ungrouped";
export const GRAPH_EDGE_BUCKET_ROWS = 256;
