import { memo, useEffect, useMemo, useRef, useState } from "react";
import type { CommitInfo } from "../api";
import { useI18n } from "../i18n";
import { GRAPH_EDGE_BUCKET_ROWS, shortOid, type RunOperation } from "../types";
import { RowMenu } from "./RepositoryPane";

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

export function HistoryCanvas({ commits, selectedOid, onSelect }: { commits: CommitInfo[]; selectedOid?: string; onSelect: (oid: string) => void }) {
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

export function HistoryPane({ commits, selectedOid, loading, hasMore, onLoadMore, onSelect, onRun }: { commits: CommitInfo[]; selectedOid?: string; loading: boolean; hasMore: boolean; onLoadMore: () => void; onSelect: (oid: string) => void; onRun: RunOperation }) {
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

export const MemoHistoryCanvas = memo(HistoryCanvas);
export const MemoHistoryPane = memo(HistoryPane);
