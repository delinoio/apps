// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { PlanningKind, type PlanningItem } from "../src/planning.ts";
import {
  loadPlanningManager,
  type HostNode,
} from "./harness/planning-manager.ts";

const repositoryId = "repo";
const common = {
  repositoryId,
  description: "",
  color: "",
  completed: false,
  startDate: "",
  endDate: "",
  conflicted: false,
};
const items: PlanningItem[] = [
  {
    ...common,
    id: "bug",
    versionId: "bug-v1",
    kind: PlanningKind.Label,
    name: "Bug",
    color: "#D64545",
  },
  {
    ...common,
    id: "feature",
    versionId: "feature-v1",
    kind: PlanningKind.Label,
    name: "Feature",
    color: "#075AFF",
    conflicted: true,
  },
  {
    ...common,
    id: "launch",
    versionId: "launch-v1",
    kind: PlanningKind.Milestone,
    name: "Launch",
    targetDate: "2026-12-01",
  },
  {
    ...common,
    id: "october",
    versionId: "october-v1",
    kind: PlanningKind.Cycle,
    name: "October",
    startDate: "2026-10-01",
    endDate: "2026-10-31",
  },
];

function find(
  roots: (HostNode | string)[],
  match: (node: HostNode) => boolean,
): HostNode[] {
  const found: HostNode[] = [];
  const visit = (node: HostNode | string) => {
    if (typeof node === "string") return;
    if (match(node)) found.push(node);
    node.children.forEach(visit);
  };
  roots.forEach(visit);
  return found;
}
const content = (node: HostNode | string): string =>
  typeof node === "string" ? node : node.children.map(content).join("");

test("planning panels preserve item semantics, variants and dirty navigation", async () => {
  const bundle = await loadPlanningManager();
  const dirty: boolean[] = [];
  const view = bundle.runtime.mount(bundle.PlanningManager, {
    adapter: { call: () => assert.fail("Selection must not write") },
    repository: {
      version: {
        entityId: repositoryId,
        body: { case: "repository", value: { displayName: "repo" } },
      },
    },
    planning: { items, status: "ready", error: "", retry: () => {} },
    refresh: () => {},
    dirty: (value: boolean) => dirty.push(value),
    back: () => {},
  } as any);
  const button = (name: string) => {
    const matches = find(
      view.tree(),
      (node) =>
        node.props["accessibility-traits"] === "button" &&
        node.props["accessibility-label"] === name,
    );
    assert.equal(matches.length, 1, name);
    return matches[0];
  };
  const tap = async (name: string) => {
    button(name).props.bindtap();
    await view.settle();
  };
  const row = (name: string) => button(`Edit ${name}`);
  const fieldNames = () =>
    find(view.tree(), (node) => ["input", "textarea"].includes(node.type)).map(
      (node) => node.props["accessibility-label"],
    );

  assert.match(content(row("label Bug")), /Bug#D64545/);
  assert.match(content(row("label Feature")), /Feature#075AFFConflict/);
  assert.equal(row("label Feature").props["data-pressed"], "false");
  assert.equal(
    find(
      [row("label Feature")],
      (node) => node.props.className === "planning-row-swatch",
    ).length,
    1,
  );
  await tap("Edit label Feature");
  assert.equal(row("label Feature").props["data-pressed"], "true");
  assert.equal(row("label Feature").props["accessibility-value"], "Selected");
  assert.ok(
    content(view.tree()[0]).includes(
      "Changes are saved only when you choose Save.",
    ),
  );
  assert.deepEqual(fieldNames(), ["Name", "Color (#RRGGBB)", "Description"]);
  assert.ok(button("History"));
  assert.ok(button("Delete"));
  assert.ok(button("Save"));
  assert.ok(button("Close"));

  const name = find(
    view.tree(),
    (node) =>
      node.type === "input" && node.props["accessibility-label"] === "Name",
  )[0];
  name.props.bindinput({ detail: { value: "Edited Feature" } });
  await view.settle();
  assert.equal(dirty.at(-1), true);
  await tap("Edit label Bug");
  assert.equal(row("label Feature").props["data-pressed"], "true");
  assert.ok(content(view.tree()[0]).includes("Discard unsaved changes?"));
  await tap("Keep editing");
  assert.equal(row("label Feature").props["data-pressed"], "true");
  await tap("Edit label Bug");
  await tap("Discard changes");
  assert.equal(row("label Bug").props["data-pressed"], "true");
  assert.equal(dirty.at(-1), false);

  await tap("New label");
  assert.deepEqual(fieldNames(), ["Name", "Color (#RRGGBB)", "Description"]);
  assert.equal(
    find(view.tree(), (node) => node.props["accessibility-label"] === "History")
      .length,
    0,
  );
  assert.equal(
    find(view.tree(), (node) => node.props["accessibility-label"] === "Delete")
      .length,
    0,
  );

  await tap("Milestones tab");
  assert.match(
    content(row("milestone Launch")),
    /LaunchOpen · Target 2026-12-01/,
  );
  await tap("Edit milestone Launch");
  assert.deepEqual(fieldNames(), [
    "Name",
    "Target date (YYYY-MM-DD, optional)",
    "Description",
  ]);
  assert.ok(
    content(view.tree()[0]).includes(
      "Completing a milestone does not change task progress.",
    ),
  );

  await tap("Cycles tab");
  assert.match(
    content(row("cycle October")),
    /October2026-10-01 to 2026-10-31/,
  );
  await tap("Edit cycle October");
  assert.deepEqual(fieldNames(), [
    "Name",
    "Start date (YYYY-MM-DD)",
    "End date (YYYY-MM-DD)",
    "Description",
  ]);
  view.unmount();
});
