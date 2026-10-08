import { expect, test } from "vitest";
import type { BranchInfo } from "../api";
import { bestScore, matchScore, rankBranches, rankOrder, searchName, TIER } from "./search";

const branch = (name: string, extra: Partial<BranchInfo> = {}): BranchInfo => ({ name, oid: "a".repeat(40), current: false, remote: false, upstream: null, committedAt: null, ...extra });

test("grades exact, prefix, segment and substring matches", () => {
  expect(matchScore("develop", "develop")?.tier).toBe(TIER.exact);
  expect(matchScore("develop-2", "develop")?.tier).toBe(TIER.prefix);
  expect(matchScore("feature/develop", "develop")?.tier).toBe(TIER.segment);
  expect(matchScore("dev-develop", "develop")?.tier).toBe(TIER.segment);
  expect(matchScore("mydevelop", "develop")?.tier).toBe(TIER.substring);
  expect(matchScore("main", "develop")).toBeNull();
});

test("ignores query case and surrounding whitespace", () => {
  expect(matchScore("DEVELOP", " develop ")?.tier).toBe(TIER.exact);
  expect(matchScore("Develop-2", " DeVeLoP ")?.tier).toBe(TIER.prefix);
});

test("keeps the best occurrence when a value matches more than once", () => {
  expect(matchScore("develop-develop", "develop")).toMatchObject({ tier: TIER.prefix, index: 0 });
});

test("ranks by match quality across fields", () => {
  expect(matchScore("develop", "develop")!.field).toBe(0);
  const score = bestScore(["mydevelop", "develop"], "develop")!;
  expect(score).toMatchObject({ tier: TIER.exact, field: 1 });
});

test("keeps the incoming order and identity for an empty query", () => {
  const items = ["b", "a"];
  expect(rankOrder(items, "  ", (item) => [item])).toBe(items);
  expect(rankOrder(items, "", (item) => [item])).toBe(items);
});

test("orders exact matches before prefix, segment and substring matches", () => {
  const values = ["develop-xxxx-xxxx-xxx", "origin/develop-two", "feature/develop", "mydevelop", "develop"];
  expect(rankOrder(values, "develop", (value) => [value])).toEqual(["develop", "develop-xxxx-xxxx-xxx", "feature/develop", "origin/develop-two", "mydevelop"]);
});

test("breaks ties with the supplied tiebreak and then the original order", () => {
  const items = [{ name: "develop-b", rank: 1 }, { name: "develop-a", rank: 2 }, { name: "develop-c", rank: 2 }];
  expect(rankOrder(items, "develop", (item) => [item.name], (left, right) => right.rank - left.rank).map((item) => item.name)).toEqual(["develop-a", "develop-c", "develop-b"]);
  expect(rankOrder(items, "develop", (item) => [item.name]).map((item) => item.name)).toEqual(["develop-b", "develop-a", "develop-c"]);
});

test("strips the remote prefix before scoring so origin/develop counts as an exact match", () => {
  expect(searchName(branch("origin/develop", { remote: true }))).toBe("develop");
  expect(searchName(branch("origin/team/topic", { remote: true }))).toBe("team/topic");
  expect(searchName(branch("develop"))).toBe("develop");
});

test("ranks a local develop ahead of longer develop branches, and origin/develop as an exact match", () => {
  const branches = [
    branch("develop-xxxx-xxxx-xxx", { current: true }),
    branch("develop-2"),
    branch("origin/develop", { remote: true }),
    branch("develop"),
    branch("feature/develop"),
  ];
  expect(rankBranches(branches, "develop").map((entry) => entry.name)).toEqual(["develop", "origin/develop", "develop-2", "develop-xxxx-xxxx-xxx", "feature/develop"]);
});

test("prefers the current branch only when match quality is otherwise equal", () => {
  const branches = [branch("develop-1"), branch("develop-2", { current: true })];
  expect(rankBranches(branches, "develop").map((entry) => entry.name)).toEqual(["develop-2", "develop-1"]);
});

test("ranks a closer same-tier match ahead of the current branch", () => {
  const branches = [branch("develop-xxxx-xxxx-xxx", { current: true }), branch("develop-2")];
  expect(rankBranches(branches, "develop").map((entry) => entry.name)).toEqual(["develop-2", "develop-xxxx-xxxx-xxx"]);
});

test("orders equally relevant branches deterministically by name", () => {
  const branches = [branch("develop-b"), branch("develop-a")];
  expect(rankBranches(branches, "develop").map((entry) => entry.name)).toEqual(["develop-a", "develop-b"]);
});
