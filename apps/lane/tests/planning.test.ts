// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { create } from "@bufbuild/protobuf";
import {
  EntityResultSchema,
  EntityVersionSchema,
  RelationshipKind,
  TaskPriority,
  TaskSchema,
  TaskType,
} from "@delino/protos/lane/v1/lane_pb";
import { LaneError } from "../src/adapter.ts";
import { toUI } from "../src/serializable.ts";
import { historyChanges } from "../src/model.ts";
import {
  PlanningKind,
  activeFilterCount,
  deriveRelations,
  describeError,
  describeItemError,
  emptyFilter,
  filterErrors,
  lookupOf,
  parsePoints,
  planningItemChanges,
  planningItemOf,
  planningBody,
  blankPlanningInput,
  pruneFilter,
  relationshipEdits,
  taskMetadata,
  toTaskFilter,
  unionRelated,
  validDate,
  validatePlanningInput,
  validateTaskPlanning,
  type PlanningItem,
  type TaskRef,
} from "../src/planning.ts";

const uiTask = (patch: Record<string, unknown> = {}) =>
  toUI(create(TaskSchema, { title: "t", ...patch }));
const item = (patch: Partial<PlanningItem>): PlanningItem => ({
  id: "i",
  versionId: "v",
  kind: PlanningKind.Label,
  repositoryId: "r1",
  name: "Name",
  description: "",
  color: "#075AFF",
  completed: false,
  startDate: "",
  endDate: "",
  conflicted: false,
  ...patch,
});

test("calendar dates are real dates including leap years", () => {
  assert.equal(validDate("2028-02-29"), true);
  assert.equal(validDate("2100-02-29"), false);
  assert.equal(validDate("2000-02-29"), true);
  assert.equal(validDate("2026-13-01"), false);
  assert.equal(validDate("2026-04-31"), false);
  assert.equal(validDate("2026-4-3"), false);
  assert.equal(validDate("2026-10-26T00:00:00Z"), false);
});

test("story points: zero differs from unassigned and bounds are exact", () => {
  assert.equal(parsePoints(""), undefined);
  assert.equal(parsePoints("0"), 0);
  assert.equal(parsePoints("2147483647"), 2147483647);
  for (const bad of ["2147483648", "-1", "1.5", "1e3", "abc", "０"])
    assert.equal(typeof parsePoints(bad), "string", bad);
});

test("task planning validation explains date order, range and label limit", () => {
  assert.deepEqual(
    validateTaskPlanning(
      { startDate: "2026-02-01", dueDate: "2026-02-01", labelIds: [] },
      "0",
    ),
    {},
  );
  assert.match(
    validateTaskPlanning(
      { startDate: "2026-03-02", dueDate: "2026-03-01", labelIds: [] },
      "",
    ).startDate!,
    /on or before the due date/,
  );
  assert.match(
    validateTaskPlanning({ startDate: "2026-02-30", labelIds: [] }, "")
      .startDate!,
    /YYYY-MM-DD/,
  );
  assert.match(
    validateTaskPlanning({ labelIds: [] }, "2147483648").storyPoints!,
    /whole number/,
  );
  assert.match(
    validateTaskPlanning(
      { labelIds: Array.from({ length: 101 }, (_, i) => `l${i}`) },
      "",
    ).labels!,
    /at most 100/,
  );
});

test("typed constraint violations are explained in text, from detail or sync reason", () => {
  const byDetail = new LaneError({
    reason: "ERROR_REASON_CONSTRAINT_VIOLATION",
    message: "failed",
    violation: "CONSTRAINT_VIOLATION_PARENT_CYCLE",
  });
  assert.match(describeError(byDetail), /loop/);
  assert.match(describeError(byDetail), /not saved/);
  const bySync = new LaneError({
    reason: "CONSTRAINT_VIOLATION",
    message: "CONSTRAINT_VIOLATION:DUPLICATE_NAME",
  });
  assert.match(describeError(bySync), /already uses this name/);
  assert.match(
    describeError(
      new LaneError({ reason: "CONFLICT_READ_ONLY", message: "x" }),
    ),
    /unresolved conflict/,
  );
  assert.equal(describeError(new Error("plain")), "plain");
});

test("filters: no condition sends nothing; each field maps to its typed range and unassigned flag", () => {
  assert.equal(toTaskFilter(emptyFilter()), undefined);
  const filter = emptyFilter();
  filter.priorities = [TaskPriority.URGENT, TaskPriority.LOW];
  filter.unassignedPriority = true;
  filter.types = [TaskType.BUG];
  filter.labelIds = ["l1"];
  filter.unlabeled = true;
  filter.startFrom = "2026-01-01";
  filter.dueUnassigned = true;
  filter.pointsMin = "0";
  filter.pointsMax = "8";
  filter.noMilestone = true;
  filter.cycleIds = ["c1"];
  filter.relationships = [RelationshipKind.PARENT, RelationshipKind.BLOCKS];
  filter.noRelationships = true;
  const sent = toTaskFilter(filter)!;
  assert.deepEqual(sent.priorities, [TaskPriority.URGENT, TaskPriority.LOW]);
  assert.equal(sent.includeUnassignedPriority, true);
  assert.deepEqual(sent.types, [TaskType.BUG]);
  assert.equal(sent.includeUnlabeled, true);
  assert.equal(sent.startDate?.from, "2026-01-01");
  assert.equal(sent.startDate?.to, undefined);
  assert.equal(sent.startDate?.includeUnassigned, false);
  assert.deepEqual(
    [sent.dueDate?.from, sent.dueDate?.includeUnassigned],
    [undefined, true],
  );
  assert.deepEqual([sent.storyPoints?.min, sent.storyPoints?.max], [0, 8]);
  assert.equal(sent.includeNoMilestone, true);
  assert.deepEqual(sent.cycleIds, ["c1"]);
  assert.deepEqual(sent.relationships, [
    RelationshipKind.PARENT,
    RelationshipKind.BLOCKS,
  ]);
  assert.equal(sent.includeNoRelationships, true);
  assert.equal(activeFilterCount(filter), 9);
});

test("filter validation blocks malformed ranges instead of sending them", () => {
  const filter = emptyFilter();
  filter.startFrom = "2026-02-30";
  filter.dueFrom = "2026-05-02";
  filter.dueTo = "2026-05-01";
  filter.pointsMin = "9";
  filter.pointsMax = "3";
  assert.deepEqual(Object.keys(filterErrors(filter)).sort(), [
    "due",
    "points",
    "start",
  ]);
  filter.pointsMin = "1.5";
  assert.match(filterErrors(filter).points!, /whole numbers/);
  assert.deepEqual(filterErrors(emptyFilter()), {});
});

test("changing repository scope prunes unavailable repository-owned selections", () => {
  const filter = emptyFilter();
  filter.labelIds = ["a", "b"];
  filter.priorities = [TaskPriority.HIGH];
  const pruned = pruneFilter(filter, [item({ id: "a" })]);
  assert.deepEqual(pruned.labelIds, ["a"]);
  assert.deepEqual(pruned.priorities, [TaskPriority.HIGH]);
  assert.equal(pruneFilter(pruned, [item({ id: "a" })]), pruned);
});

test("subtasks and blocks are derived inverse views and produce minimal atomic edits", () => {
  const ref = (id: string, patch: Record<string, unknown>): TaskRef => ({
    id,
    title: id,
    task: uiTask(patch),
  });
  const tasks = [
    ref("child", { parentTaskId: "me" }),
    ref("waiting", { dependsOnTaskIds: ["me", "other"] }),
    ref("peer", { relatedTaskIds: ["me"] }),
    ref("other", {}),
  ];
  const relations = deriveRelations("me", tasks);
  assert.deepEqual(relations.subtasks, ["child"]);
  assert.deepEqual(relations.blocks, ["waiting"]);
  assert.deepEqual(unionRelated("me", ["x"], tasks), ["peer", "x"]);
  assert.equal(relationshipEdits(relations), undefined);
  assert.deepEqual(
    relationshipEdits({
      ...relations,
      subtasks: ["other"],
      blocks: ["waiting", "peer"],
    }),
    {
      addSubtaskIds: ["other"],
      removeSubtaskIds: ["child"],
      addBlockedTaskIds: ["peer"],
      removeBlockedTaskIds: [],
    },
  );
});

test("list metadata qualifies repository-owned values only when asked", () => {
  const label = item({ id: "l", name: "Backend", repositoryId: "r1" });
  const milestone = item({ id: "m", kind: PlanningKind.Milestone, name: "v1" });
  const lookup = lookupOf(
    [label, milestone],
    new Map(),
    new Map([["r1", "delinoio/apps"]]),
  );
  const task = uiTask({
    priority: TaskPriority.HIGH,
    labelIds: ["l", "gone"],
    milestoneId: "m",
    storyPoints: 0,
    dueDate: "2026-11-05",
    parentTaskId: "p",
    relatedTaskIds: ["a", "b"],
  });
  const plain = taskMetadata(task, lookup, false);
  assert.deepEqual(plain.summary, [
    "Priority High",
    "Due 2026-11-05",
    "0 points",
    "Milestone v1",
  ]);
  assert.deepEqual(plain.labels, ["Backend", "(unavailable label)"]);
  assert.deepEqual(plain.relationships, ["Has parent", "Related 2"]);
  const qualified = taskMetadata(task, lookup, true);
  assert.deepEqual(qualified.labels[0], "delinoio/apps / Backend");
  assert.equal(qualified.summary.at(-1), "Milestone delinoio/apps / v1");
});

test("planning items: validation, case-insensitive duplicate names and typed bodies", () => {
  const siblings = [item({ id: "a", name: "Bug" })];
  const input = { ...blankPlanningInput(), name: "  bug " };
  assert.match(
    validatePlanningInput(PlanningKind.Label, input, siblings).name!,
    /already uses this name/,
  );
  assert.deepEqual(
    validatePlanningInput(PlanningKind.Label, input, siblings, "a"),
    {},
  );
  assert.match(
    validatePlanningInput(
      PlanningKind.Label,
      { ...input, name: "x", color: "red" },
      [],
    ).color!,
    /#RRGGBB/,
  );
  const cycle = {
    ...blankPlanningInput(),
    name: "S1",
    startDate: "2026-02-02",
    endDate: "2026-02-01",
  };
  assert.match(
    validatePlanningInput(PlanningKind.Cycle, cycle, []).endDate!,
    /on or after/,
  );
  assert.match(
    validatePlanningInput(PlanningKind.Cycle, { ...cycle, startDate: "" }, [])
      .startDate!,
    /Enter a start date/,
  );
  assert.equal(
    validatePlanningInput(
      PlanningKind.Label,
      { ...input, name: "𝒳".repeat(101) },
      [],
    ).name,
    "A name can have at most 100 characters.",
  );
  assert.deepEqual(
    planningBody(PlanningKind.Label, "r1", { ...input, color: "#0a5aff" }),
    {
      body: {
        case: "label",
        value: {
          repositoryId: "r1",
          name: "bug",
          description: "",
          color: "#0A5AFF",
        },
      },
    },
  );
});

const planningVersion = (id: string, name: string, color = "#111111") =>
  toUI(
    create(EntityVersionSchema, {
      entityId: "label-1",
      versionId: id,
      body: {
        case: "label",
        value: { repositoryId: "r1", name, color, description: "d" },
      },
    }),
  );

test("planning item history shows before and after for changed fields only", () => {
  const changes = planningItemChanges(
    planningVersion("v1", "Bug"),
    planningVersion("v2", "Defect"),
  );
  assert.deepEqual(changes, [
    { field: "Name", before: "Bug", after: "Defect" },
  ]);
  assert.equal(
    planningItemChanges(undefined, planningVersion("v1", "Bug")).every(
      (c) => c.before === "(new item)",
    ),
    true,
  );
  const result = toUI(
    create(EntityResultSchema, {
      version: create(EntityVersionSchema, {
        entityId: "m",
        versionId: "v",
        body: {
          case: "milestone",
          value: { repositoryId: "r1", name: "M", completed: true },
        },
      }),
    }),
  );
  assert.deepEqual(
    [planningItemOf(result)?.kind, planningItemOf(result)?.completed],
    [PlanningKind.Milestone, true],
  );
});

test("task history names planning changes from current lookups and hides deleted names", () => {
  const taskVersion = (id: string, patch: Record<string, unknown>) =>
    create(EntityVersionSchema, {
      entityId: "t",
      versionId: id,
      body: {
        case: "task",
        value: create(TaskSchema, { title: "t", ...patch }),
      },
    });
  const lookup = lookupOf(
    [item({ id: "l1", name: "Backend" })],
    new Map([["p", "Parent task"]]),
    new Map(),
  );
  const diff = historyChanges(
    taskVersion("a", {}),
    taskVersion("b", {
      priority: TaskPriority.URGENT,
      labelIds: ["l1", "deleted"],
      parentTaskId: "p",
      storyPoints: 0,
    }),
    lookup,
  );
  const byField = Object.fromEntries(diff.map((c) => [c.field, c]));
  assert.equal(byField.Priority.after, "Urgent");
  assert.equal(byField.Labels.after, "Backend, (deleted item)");
  assert.equal(byField.Parent.after, "Parent task");
  assert.deepEqual(
    [byField["Story points"].before, byField["Story points"].after],
    ["(none)", "0"],
  );
  assert.equal(
    diff.some((c) => c.field === "Type"),
    false,
  );
  assert.equal(
    historyChanges(taskVersion("a", {}), taskVersion("b", {})).length,
    0,
  );
});

test("rejected synchronization reasons are explained and omitted ProtoJSON defaults are tolerated", () => {
  assert.match(
    describeItemError("CONSTRAINT_VIOLATION:DUPLICATE_NAME"),
    /retry the blocked items/,
  );
  assert.equal(describeItemError("STORAGE_FULL"), "STORAGE_FULL");
  const sparse = toUI(
    create(EntityVersionSchema, {
      entityId: "m",
      versionId: "v",
      body: { case: "milestone", value: { repositoryId: "r1" } },
    }),
  );
  const parsed = planningItemOf(
    toUI(
      create(EntityResultSchema, {
        version: create(EntityVersionSchema, {
          entityId: "m",
          versionId: "v",
          body: { case: "milestone", value: { repositoryId: "r1" } },
        }),
      }),
    ),
  )!;
  assert.deepEqual(
    [parsed.name, parsed.description, parsed.completed, parsed.targetDate],
    ["", "", false, undefined],
  );
  // Even hand-built bodies without the empty fields default safely.
  delete (sparse.body.value as Record<string, unknown>).name;
  delete (sparse.body.value as Record<string, unknown>).completed;
  assert.equal(
    planningItemOf({
      result: 1,
      alternatives: [],
      correlationId: "",
      $typeName: "lane.v1.EntityResult",
      version: sparse,
    } as never)!.completed,
    false,
  );
});
