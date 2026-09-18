import type { BranchInfo } from "../api";

export const TIER = { exact: 0, prefix: 1, segment: 2, substring: 3 } as const;
export type SearchTier = (typeof TIER)[keyof typeof TIER];
export type SearchScore = { tier: SearchTier; field: number; index: number; length: number };

const SEGMENT_EDGE = /[/\-_. :]/;

const compareScores = (left: SearchScore, right: SearchScore) =>
  left.tier - right.tier || left.field - right.field || left.length - right.length || left.index - right.index;

export const normalizeQuery = (query: string) => query.trim().toLowerCase();

export function matchScore(value: string, query: string): SearchScore | null {
  const haystack = value.toLowerCase();
  const needle = normalizeQuery(query);
  if (!needle) return null;
  let best: SearchScore | null = null;
  for (let index = haystack.indexOf(needle); index >= 0; index = haystack.indexOf(needle, index + 1)) {
    const tier: SearchTier = index > 0
      ? SEGMENT_EDGE.test(haystack[index - 1]) ? TIER.segment : TIER.substring
      : haystack.length === needle.length ? TIER.exact : TIER.prefix;
    if (!best || tier < best.tier) best = { tier, field: 0, index, length: haystack.length };
    if (best.tier === TIER.exact) break;
  }
  return best;
}

export function bestScore(values: Array<string | null | undefined>, query: string): SearchScore | null {
  let best: SearchScore | null = null;
  for (let field = 0; field < values.length; field += 1) {
    const value = values[field];
    if (!value) continue;
    const score = matchScore(value, query);
    if (!score) continue;
    const candidate = { ...score, field };
    if (!best || compareScores(candidate, best) < 0) best = candidate;
  }
  return best;
}

export function rankOrder<T>(items: T[], query: string, values: (item: T) => Array<string | null | undefined>, tiebreak?: (left: T, right: T) => number): T[] {
  if (!normalizeQuery(query)) return items;
  return items
    .map((item, index) => ({ item, index, score: bestScore(values(item), query) }))
    .sort((left, right) => {
      if (!left.score || !right.score) return Number(!left.score) - Number(!right.score);
      return compareScores(left.score, right.score) || (tiebreak?.(left.item, right.item) ?? 0) || left.index - right.index;
    })
    .map((entry) => entry.item);
}

export const searchName = (branch: BranchInfo) => branch.remote ? branch.name.slice(branch.name.indexOf("/") + 1) : branch.name;

export function rankBranches(branches: BranchInfo[], query: string): BranchInfo[] {
  return rankOrder(
    branches,
    query,
    (branch) => [searchName(branch), branch.name],
    (left, right) => Number(right.current) - Number(left.current) || Number(left.remote) - Number(right.remote) || searchName(left).localeCompare(searchName(right)),
  );
}
