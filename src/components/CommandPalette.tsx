import { useEffect, useRef, useState } from "react";
import { useI18n } from "../i18n";
import type { CommandItem } from "../types";

export function CommandPalette({ items, onClose }: { items: CommandItem[]; onClose: () => void }) {
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
