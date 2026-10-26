// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { create } from "@bufbuild/protobuf";
import {
  EntityResultSchema,
  EntityVersionSchema,
  TaskSchema,
  Progress,
  Proposal,
  ResultKind,
} from "@repo/protos/lane/v1/lane_pb";
import {
  draftFor,
  receiveExternal,
  historyChanges,
  historyBaseline,
  historyEntryChanges,
  heads,
  conflicted,
  headIds,
  sameHeadSet,
  resolutionStale,
} from "../src/model.ts";
const version = (id: string, title: string, progress = Progress.TODO) =>
  create(EntityVersionSchema, {
    entityId: "task",
    versionId: id,
    body: {
      case: "task",
      value: create(TaskSchema, {
        title,
        progress,
        proposal: Proposal.PENDING,
        prompt: "original",
      }),
    },
  });
test("external updates retain unsaved title, description, prompt, progress and original base", () => {
  const initial = create(EntityResultSchema, { version: version("v1", "old") });
  const draft = draftFor(initial);
  draft.task.title = "unsaved";
  draft.task.description = "unsaved description";
  draft.task.prompt = "unsaved prompt";
  draft.dirty = true;
  const next = receiveExternal(
    draft,
    create(EntityResultSchema, {
      version: version("v2", "remote", Progress.DONE),
    }),
  );
  assert.equal(next.task, draft.task);
  assert.equal(next.base, "v1");
  assert.equal(next.external, true);
});
test("deletion blocks future editing but retains current-screen source", () => {
  const draft = draftFor(
    create(EntityResultSchema, { version: version("v1", "retain me") }),
  );
  const next = receiveExternal(
    draft,
    create(EntityResultSchema, { result: ResultKind.DELETED }),
  );
  assert.equal(next.deleted, true);
  assert.equal(next.task.title, "retain me");
});
test("all progress transitions stay distinct from proposal disposition in history", () => {
  for (const before of [1, 2, 3, 4, 5])
    for (const after of [1, 2, 3, 4, 5]) {
      const a = version("a", "title", before),
        b = version("b", "title", after);
      const diff = historyChanges(a, b);
      assert.equal(
        diff.some((d) => d.field === "progress"),
        before !== after,
      );
      assert.equal(
        diff.some((d) => d.field === "proposal"),
        false,
      );
    }
});
test("whole-task conflict includes every unique head", () => {
  const a = version("a", "one"),
    b = version("b", "two");
  const r = create(EntityResultSchema, {
    result: ResultKind.CONFLICT,
    version: a,
    alternatives: [a, b],
  });
  assert.equal(conflicted(r), true);
  assert.deepEqual(
    heads(r).map((h) => h.versionId),
    ["a", "b"],
  );
});
test("a resolution stays bound to the reviewed head set and treats order as irrelevant", () => {
  const conflict = (...ids: string[]) =>
    create(EntityResultSchema, {
      result: ids.length > 1 ? ResultKind.CONFLICT : ResultKind.APPLIED,
      version: version(ids[0], "t"),
      alternatives: ids.slice(1).map((id) => version(id, "t")),
    });
  const reviewed = headIds(conflict("h2", "h1"));
  assert.deepEqual(reviewed, ["h1", "h2"]);
  assert.ok(sameHeadSet(reviewed, ["h2", "h1"]));
  assert.ok(!sameHeadSet(reviewed, ["h1", "r"]));
  // Unchanged or merely reordered heads keep the session current.
  assert.equal(resolutionStale(reviewed, conflict("h1", "h2")), false);
  assert.equal(resolutionStale(reviewed, conflict("h2", "h1")), false);
  // A different conflict set, a partial overlap and the single-head interval are stale.
  assert.equal(resolutionStale(reviewed, conflict("r", "s")), true);
  assert.equal(resolutionStale(reviewed, conflict("h1", "s")), true);
  assert.equal(resolutionStale(reviewed, conflict("h1", "h2", "s")), true);
  assert.equal(resolutionStale(reviewed, conflict("r")), true);
  assert.equal(resolutionStale(reviewed, undefined), true);
  // Without a session there is nothing to invalidate.
  assert.equal(resolutionStale(undefined, conflict("r", "s")), false);
});
test("history baselines follow declared parents", () => {
  const entry = (id: string, parents: string[]) =>
    create(EntityVersionSchema, {
      ...version(id, id),
      versionId: id,
      parentVersionIds: parents,
    });
  const a = entry("a", []);
  const b = entry("b", ["a"]);
  const c = entry("c", ["a"]);
  const r = entry("r", ["b", "c"]);
  const loaded = [b, c];
  assert.equal(historyBaseline(loaded, a).kind, "creation");
  assert.equal(historyBaseline(loaded, c).kind, "pending");
  assert.equal(historyBaseline([a, b, c], c).kind, "parent");
  assert.equal(historyBaseline([a, b, c], r).kind, "merge");
  assert.deepEqual(historyEntryChanges(loaded, c), []);
  assert.deepEqual(historyEntryChanges([a, b, c, r], r), []);
  assert.deepEqual(historyEntryChanges([a, b, c], c), historyChanges(a, c));
});
