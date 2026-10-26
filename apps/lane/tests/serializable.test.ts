// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { create, fromJson, toJson } from "@bufbuild/protobuf";
import {
  SaveReceiverRequestSchema,
  SyncMutationSchema,
  TaskSchema,
  SetProgressRequestSchema,
  EntityVersionSchema,
  ListTasksRequestSchema,
  TaskFilterSchema,
  CreatePlanningItemRequestSchema,
  SaveTaskRequestSchema,
} from "@repo/protos/lane/v1/lane_pb";
import { toUI, fromUI } from "../src/serializable.ts";
import { progressOptions } from "../src/model.ts";
test("uint64 larger than MAX_SAFE_INTEGER remains exact through UI and schema restoration", () => {
  const value = 18446744073709551615n;
  const original = create(SaveReceiverRequestSchema, {
    expectedRevision: value,
  });
  const ui = toUI(original);
  assert.equal(ui.expectedRevision, "18446744073709551615");
  assert.doesNotThrow(() => JSON.stringify(ui));
  const restored = fromUI(SaveReceiverRequestSchema, ui);
  assert.equal(restored.expectedRevision, value);
  assert.equal(
    toJson(SaveReceiverRequestSchema, restored).expectedRevision,
    "18446744073709551615",
  );
});
test("every shared sync fixture preserves immutable snapshots and prompt restoration markers through UI boundary", async () => {
  const fixtures = JSON.parse(
    await readFile(
      new URL("../../../protos/lane/v1/fixtures/sync.json", import.meta.url),
      "utf8",
    ),
  );
  let count = 0;
  for (const c of fixtures.cases) {
    for (const mutation of c.input.mutations || []) {
      const original = fromJson(SyncMutationSchema, mutation);
      assert.deepEqual(
        toJson(SyncMutationSchema, fromUI(SyncMutationSchema, toUI(original))),
        toJson(SyncMutationSchema, original),
        c.id,
      );
      count++;
    }
    for (const version of c.input.initial || []) {
      const original = fromJson(EntityVersionSchema, version);
      assert.deepEqual(
        toJson(
          EntityVersionSchema,
          fromUI(EntityVersionSchema, toUI(original)),
        ),
        toJson(EntityVersionSchema, original),
        c.id,
      );
    }
  }
  assert.ok(count > 10);
});
test("shared 25 transition vectors all have selectable GUI progress targets", async () => {
  const fixtures = JSON.parse(
    await readFile(
      new URL("../../../protos/lane/v1/fixtures/tasks.json", import.meta.url),
      "utf8",
    ),
  );
  const cases = fixtures.cases.filter((c: any) => c.id.startsWith("progress-"));
  assert.equal(cases.length, 25);
  for (const c of cases) {
    const request = fromJson(SetProgressRequestSchema, c.input.request);
    const expected = fromJson(TaskSchema, c.expected.task);
    assert.ok(progressOptions.some(([value]) => value === request.progress));
    assert.equal(request.progress, expected.progress);
    assert.deepEqual(
      toJson(
        SetProgressRequestSchema,
        fromUI(SetProgressRequestSchema, toUI(request)),
      ),
      toJson(SetProgressRequestSchema, request),
    );
  }
});
test("planning task fields round-trip through UI and ProtoJSON, keeping zero points distinct from absent", () => {
  const task = create(TaskSchema, {
    title: "planned",
    priority: 2,
    type: 5,
    labelIds: ["a", "b"],
    startDate: "2026-02-28",
    dueDate: "2026-03-01",
    storyPoints: 0,
    milestoneId: "m",
    parentTaskId: "p",
    dependsOnTaskIds: ["d"],
    relatedTaskIds: ["r"],
    duplicateOfTaskId: "o",
  });
  const ui = toUI(task);
  assert.doesNotThrow(() => JSON.stringify(ui));
  const json = toJson(TaskSchema, fromUI(TaskSchema, ui));
  assert.equal(json.priority, "TASK_PRIORITY_HIGH");
  assert.equal(json.type, "TASK_TYPE_MAINTENANCE");
  assert.equal(json.storyPoints, 0);
  assert.deepEqual(json.labelIds, ["a", "b"]);
  assert.equal("cycleId" in json, false);
  const unset = toJson(
    TaskSchema,
    fromUI(TaskSchema, toUI(create(TaskSchema, { title: "t" }))),
  );
  assert.equal("storyPoints" in unset, false);
  assert.equal("priority" in unset, false);
});
test("list filters, planning bodies and relationship edits keep their typed shape", () => {
  const list = fromUI(ListTasksRequestSchema, {
    filter: toUI(
      create(TaskFilterSchema, {
        includeNoMilestone: true,
        dueDate: { to: "2026-12-31", includeUnassigned: true },
        storyPoints: { min: 0 },
        relationships: [3, 6],
      }),
    ),
    sortKey: 4,
    sortDirection: 2,
  } as never);
  const json = toJson(ListTasksRequestSchema, list) as Record<string, any>;
  assert.equal(json.sortKey, "TASK_SORT_KEY_DUE_DATE");
  assert.equal(json.sortDirection, "SORT_DIRECTION_DESCENDING");
  assert.deepEqual(json.filter.relationships, [
    "RELATIONSHIP_KIND_DEPENDS_ON",
    "RELATIONSHIP_KIND_DUPLICATE_OF",
  ]);
  assert.deepEqual(json.filter.dueDate, {
    to: "2026-12-31",
    includeUnassigned: true,
  });
  assert.equal(json.filter.storyPoints.min, 0);
  const created = fromUI(CreatePlanningItemRequestSchema, {
    id: "x",
    item: {
      body: {
        case: "cycle",
        value: {
          repositoryId: "r",
          name: "S1",
          startDate: "2026-01-01",
          endDate: "2026-01-14",
        },
      },
    },
  } as never);
  assert.equal(created.item?.body.case, "cycle");
  assert.equal(
    (toJson(CreatePlanningItemRequestSchema, created) as any).item.cycle
      .endDate,
    "2026-01-14",
  );
  const save = fromUI(SaveTaskRequestSchema, {
    id: "t",
    expectedVersionId: "v",
    relationshipEdits: { addSubtaskIds: ["s"], removeBlockedTaskIds: ["b"] },
  } as never);
  assert.deepEqual(save.relationshipEdits?.addSubtaskIds, ["s"]);
});
