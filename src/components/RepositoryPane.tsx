import { memo, useEffect, useRef, useState } from "react";
import type { GitInfo, RepositorySummary } from "../api";
import { useI18n } from "../i18n";
import type { LogLine } from "../lib/logBuffer";
import { shortOid } from "../types";

export function EmptyState({ git, onAdd, onClone, onInit, onSelectGit, onToggleLanguage, lastLog }: { git: GitInfo; onAdd: () => void; onClone: () => void; onInit: () => void; onSelectGit: () => void; onToggleLanguage: () => void; lastLog?: LogLine }) {
  const { t } = useI18n();
  return <main className="empty-state"><div className="empty-brand"><RailMark /><span>GITDOCK / WORKSPACE</span></div><h1>{t("emptyTitle1")}<br />{t("emptyTitle2")}</h1><p>{t("emptyDescription")}</p><div className="empty-actions"><button className="primary" disabled={!git.supported} onClick={onAdd}>{t("addRepository")}</button><button disabled={!git.supported} onClick={onClone}>{t("clone")}</button><button disabled={!git.supported} onClick={onInit}>{t("initialize")}</button>{!git.supported && <button onClick={onSelectGit}>{t("selectGit")}</button>}<button onClick={onToggleLanguage}>{t("language")}</button></div><div className={`git-check ${git.supported ? "ok" : "bad"}`}><span>{git.supported ? "●" : "×"}</span><div><strong>{git.supported ? `Git ${git.version}` : t("gitRequired")}</strong><small>{git.path ?? git.error}</small></div></div>{lastLog && <p className="empty-error">{lastLog.message}</p>}</main>;
}

export function RailMark() { return <svg className="rail-mark" viewBox="0 0 32 32" aria-hidden="true"><path d="M9 4v18a6 6 0 0 0 6 6h3" /><path d="M23 4v7a5 5 0 0 1-5 5H9" /><circle cx="9" cy="4" r="2.5" /><circle cx="23" cy="4" r="2.5" /><circle cx="20" cy="28" r="2.5" /></svg>; }

function RepositoryRow({ repository, selected, draggable, dragging, shift, canMoveUp, canMoveDown, onSelect, onMove, onDragStart, onDragEnd }: { repository: RepositorySummary; selected: boolean; draggable: boolean; dragging: boolean; shift: number; canMoveUp: boolean; canMoveDown: boolean; onSelect: (repositoryId: number) => void; onMove: (direction: -1 | 1) => void; onDragStart: React.DragEventHandler<HTMLDivElement>; onDragEnd: () => void }) {
  const { t } = useI18n();
  const state = repository.kind === "missing" ? "missing" : repository.conflictCount ? "conflict" : repository.changedCount ? "changed" : "clean";
  return <div role="listitem" className={`repo-row-shell${dragging ? " dragging" : ""}`} data-repository-id={repository.id} draggable={draggable} onDragStart={onDragStart} onDragEnd={onDragEnd} style={shift ? { transform: `translateY(${shift}px)` } : undefined}><button className={`repo-row ${selected ? "selected" : ""}`} aria-current={selected ? "true" : undefined} onClick={() => onSelect(repository.id)}><span className={`status-rail ${state}`} aria-hidden="true" /><span className="repo-copy"><span className="repo-name"><span>{repository.favorite && "★ "}{repository.name}</span><i>{repository.conflictCount ? `${repository.conflictCount} ${t("conflicts")}` : t(state)}</i></span><span className="repo-meta"><code>{repository.branch || shortOid(repository.headOid)}</code><span>{repository.changedCount ? `±${repository.changedCount}` : t("clean")}</span>{(repository.ahead || repository.behind) ? <span>↑{repository.ahead} ↓{repository.behind}</span> : null}</span></span></button><RowMenu><button disabled={!canMoveUp} onClick={() => onMove(-1)}>{t("moveUp")}</button><button disabled={!canMoveDown} onClick={() => onMove(1)}>{t("moveDown")}</button></RowMenu></div>;
}

export function RowMenu({ children, label, context }: { children: React.ReactNode; label?: string; context?: boolean }) {
  const { t } = useI18n();
  const actualLabel = label ?? t("moreActions");
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const openAt = (anchor: { top: number; bottom: number; left: number; right: number }, alignStart = false) => {
    const menu = menuRef.current;
    if (!menu) return;
    menu.showPopover();
    menu.style.height = "0";
    menu.style.height = `${Math.min(menu.scrollHeight + 2, window.innerHeight - 8)}px`;
    const bounds = menu.getBoundingClientRect();
    const top = anchor.bottom + bounds.height + 4 <= window.innerHeight ? anchor.bottom + 4 : Math.max(4, anchor.top - bounds.height - 4);
    menu.style.left = `${Math.max(4, Math.min(alignStart ? anchor.left : anchor.right - bounds.width, window.innerWidth - bounds.width - 4))}px`;
    menu.style.top = `${top}px`;
    menu.focus();
  };
  const toggle = () => {
    const button = buttonRef.current;
    if (!button) return;
    if (open) { menuRef.current?.hidePopover(); return; }
    openAt(button.getBoundingClientRect());
  };
  useEffect(() => {
    const row = context ? (menuRef.current?.closest(".file-row, .object-action-row, .repo-row-shell") ?? menuRef.current?.parentElement) as HTMLElement | null : undefined;
    if (!row) return;
    const openMenu = (event: MouseEvent) => {
      event.preventDefault();
      if (open) menuRef.current?.hidePopover();
      const anchor = { top: event.clientY, bottom: event.clientY, left: event.clientX, right: event.clientX };
      // The right-button pointerdown already registered outside the popover, so opening now would be
      // light-dismissed by the trailing pointerup. Wait it out; keyboard menu keys open immediately.
      if (event.button === 2) document.addEventListener("pointerup", () => openAt(anchor, true), { once: true });
      else openAt(anchor, true);
    };
    row.addEventListener("contextmenu", openMenu);
    return () => row.removeEventListener("contextmenu", openMenu);
  }, [context, open]);
  return <>{(!context || label) && <button ref={buttonRef} className="row-menu-trigger" type="button" aria-label={actualLabel} aria-expanded={open} onClick={toggle}>{label ? actualLabel : "•••"}</button>}<div ref={menuRef} className="row-menu-popover" tabIndex={-1} popover="auto" onToggle={(event) => { setOpen(event.newState === "open"); if (context) (menuRef.current?.closest(".file-row, .object-action-row, .repo-row-shell") ?? menuRef.current?.parentElement)?.classList.toggle("menu-open", event.newState === "open"); if (event.newState === "closed" && menuRef.current?.contains(document.activeElement)) buttonRef.current?.focus(); }} onClick={(event) => { if ((event.target as HTMLElement).closest("button")) menuRef.current?.hidePopover(); }}>{children}</div></>;
}

export const MemoRepositoryRow = memo(RepositoryRow);
