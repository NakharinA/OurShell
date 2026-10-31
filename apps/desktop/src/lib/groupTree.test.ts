import { describe, expect, it } from "vitest";
import { descendantIds, groupPath, hostCount, parentCandidates } from "./groupTree";
import { emptyHost, type Group, type Host, type ItemRecord } from "./types";

const g = (id: string, parent: string | null): ItemRecord<Group> => ({
  id,
  kind: "group",
  item: { name: id, parent_id: parent },
});
const h = (id: string, group: string | null): ItemRecord<Host> => ({
  id,
  kind: "host",
  item: { ...emptyHost(), name: id, group_id: group },
});

// prod > web > edge, prod > db, lab
const groups = [g("prod", null), g("web", "prod"), g("edge", "web"), g("db", "prod"), g("lab", null)];

describe("groupTree", () => {
  it("finds descendants", () => {
    expect([...descendantIds(groups, "prod")].sort()).toEqual(["db", "edge", "web"]);
    expect(descendantIds(groups, "edge").size).toBe(0);
  });

  it("never offers a group or its descendants as its own parent", () => {
    expect(parentCandidates(groups, "web").map((x) => x.id)).toEqual(["prod", "db", "lab"]);
    expect(parentCandidates(groups, null)).toHaveLength(5);
  });

  it("builds a root-first path", () => {
    expect(groupPath(groups, "edge").map((x) => x.id)).toEqual(["prod", "web", "edge"]);
    expect(groupPath(groups, null)).toEqual([]);
  });

  it("survives cycles in stored data", () => {
    const loop = [g("a", "b"), g("b", "a")];
    expect(groupPath(loop, "a").map((x) => x.id)).toEqual(["b", "a"]);
    expect([...descendantIds(loop, "a")]).toEqual(["b"]);
  });

  it("counts hosts recursively", () => {
    const hosts = [h("1", "edge"), h("2", "db"), h("3", null), h("4", "lab")];
    expect(hostCount(groups, hosts, "prod")).toBe(2);
    expect(hostCount(groups, hosts, "web")).toBe(1);
  });
});
