// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { planningItemFromVersion } from "../src/planning.ts";
import {
  loadPlanningManager,
  type HostNode,
} from "./harness/planning-manager.ts";

type Kind = "label" | "milestone" | "cycle";
type Version = NonNullable<Parameters<typeof planningItemFromVersion>[0]>;
const repositoryId = "repo";
const kindFields = {
  label: { name: "Base", description: "", color: "#111111" },
  milestone: {
    name: "Base",
    description: "",
    targetDate: "2026-11-01",
    completed: false,
  },
  cycle: {
    name: "Base",
    description: "",
    startDate: "2026-11-01",
    endDate: "2026-11-07",
  },
};
const changedFields = {
  label: { color: "#222222" },
  milestone: { targetDate: "2026-12-01" },
  cycle: { endDate: "2026-11-14" },
};
const changedRows = {
  label: ["Color", "Before: #111111", "After: #222222"],
  milestone: ["Target date", "Before: 2026-11-01", "After: 2026-12-01"],
  cycle: ["End date", "Before: 2026-11-07", "After: 2026-11-14"],
};
const version = (
  kind: Kind,
  id: string,
  parents: string[],
  fields: Record<string, unknown> = {},
): Version =>
  ({
    entityId: `${kind}-id`,
    versionId: id,
    parentVersionIds: parents,
    createdAt: { seconds: String(1790000000 + Number(id.slice(1)) * 60) },
    origin: { kind: 1, deviceName: `Device ${id}`, accountId: "" },
    body: {
      case: kind,
      value: { repositoryId, ...kindFields[kind], ...fields },
    },
  }) as Version;

const content = (node: HostNode | string): string =>
  typeof node === "string" ? node : node.children.map(content).join("");
function find(
  roots: (HostNode | string)[],
  match: (node: HostNode) => boolean,
): HostNode[] {
  const found: HostNode[] = [];
  const walk = (node: HostNode | string) => {
    if (typeof node === "string") return;
    if (match(node)) found.push(node);
    node.children.forEach(walk);
  };
  roots.forEach(walk);
  return found;
}

const bundle = await loadPlanningManager();
async function open(kind: Kind, pages: Version[][]) {
  const current = pages[0].at(-1)!;
  const item = planningItemFromVersion(current)!;
  const calls: string[] = [];
  let failNext = false;
  const adapter = {
    call: async (
      method: { name: string },
      request: { page: { cursor: string } },
    ) => {
      assert.equal(method.name, "ListHistory");
      calls.push(request.page.cursor);
      if (failNext) {
        failNext = false;
        throw new Error("History page unavailable");
      }
      const page = request.page.cursor ? 1 : 0;
      return {
        versions: pages[page] ?? [],
        nextCursor: page + 1 < pages.length ? "next" : "",
      };
    },
  };
  const view = bundle.runtime.mount(bundle.PlanningManager, {
    adapter,
    repository: {
      version: {
        entityId: repositoryId,
        body: { case: "repository", value: { displayName: "repo" } },
      },
    },
    planning: {
      items: [item],
      status: "ready",
      error: "",
      retry: () => {},
    },
    refresh: () => {},
    dirty: () => {},
    back: () => {},
  } as any);
  const button = (name: string) => {
    const matches = find(
      view.tree(),
      (node) =>
        node.props["accessibility-traits"] === "button" &&
        node.props["accessibility-label"] === name,
    );
    assert.equal(matches.length, 1, `button ${name}`);
    return matches[0];
  };
  const tap = async (name: string) => {
    button(name).props.bindtap();
    await view.settle();
  };
  const cards = () =>
    find(view.tree(), (node) => node.props.className === "card");
  const rows = (card: HostNode) =>
    find([card], (node) => node.props.className === "history-change").map(
      (node) => find([node], (child) => child.type === "text").map(content),
    );
  if (kind !== "label")
    await tap(`${kind === "milestone" ? "Milestones" : "Cycles"} tab`);
  await tap(`Edit ${kind} ${item.name}`);
  await tap("History");
  return { view, tap, cards, rows, calls, fail: () => (failNext = true) };
}

for (const kind of ["label", "milestone", "cycle"] as const) {
  test(`${kind} history cards use declared parents for siblings, creation and resolution`, async () => {
    const root = version(kind, "v0", []);
    const renamed = version(kind, "v1", ["v0"], { name: "Renamed" });
    const sibling = version(kind, "v2", ["v0"], changedFields[kind]);
    const resolution = version(kind, "v3", ["v1", "v2"], {
      name: "Resolved",
    });
    const screen = await open(kind, [[root, renamed, sibling, resolution]]);
    const cards = screen.cards();
    assert.equal(cards.length, 4);
    assert.equal(screen.rows(cards[0]).length, kind === "label" ? 3 : 4);
    assert.ok(
      screen.rows(cards[0]).every((row) => row[1] === "Before: (new item)"),
    );
    assert.deepEqual(screen.rows(cards[1]), [
      ["Name", "Before: Base", "After: Renamed"],
    ]);
    assert.deepEqual(screen.rows(cards[2]), [changedRows[kind]]);
    assert.deepEqual(screen.rows(cards[3]), []);
    for (let i = 0; i < cards.length; i++) {
      assert.match(content(cards[i]), new RegExp(`Device v${i}`));
      assert.ok(
        content(cards[i]).includes(
          new Date((1790000000 + i * 60) * 1000).toLocaleString(),
        ),
      );
    }
    screen.view.unmount();
  });

  test(`${kind} linear history keeps its field values`, async () => {
    const root = version(kind, "v0", []);
    const child = version(kind, "v1", ["v0"], changedFields[kind]);
    const screen = await open(kind, [[root, child]]);
    assert.deepEqual(screen.rows(screen.cards()[1]), [changedRows[kind]]);
    screen.view.unmount();
  });
}

test("a missing parent gains its delta after an appended page; failed loading keeps cards", async () => {
  const unrelated = version("label", "v0", [], { name: "Unrelated" });
  const child = version("label", "v2", ["v1"], { color: "#222222" });
  const parent = version("label", "v1", [], { name: "Base" });
  const screen = await open("label", [[unrelated, child], [parent]]);
  assert.equal(screen.cards().length, 2);
  assert.deepEqual(screen.rows(screen.cards()[1]), []);
  assert.match(content(screen.cards()[1]), /Device v2/);
  screen.fail();
  await screen.tap("Load more history");
  assert.equal(screen.cards().length, 2);
  assert.deepEqual(screen.rows(screen.cards()[1]), []);
  assert.ok(
    find(screen.view.tree(), (node) =>
      content(node).includes("History page unavailable"),
    ).length > 0,
  );
  await screen.tap("Load more history");
  assert.deepEqual(screen.calls, ["", "next", "next"]);
  assert.equal(screen.cards().length, 3);
  assert.deepEqual(screen.rows(screen.cards()[1]), [changedRows.label]);
  assert.match(content(screen.cards()[1]), /Device v2/);
  screen.view.unmount();
});
