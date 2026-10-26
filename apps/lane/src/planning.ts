// SPDX-License-Identifier: Apache-2.0
// Task planning model: option vocabularies, repository planning items, list
// filters/sort, relationship derivation, validation and error explanations.
// Everything here is pure so it can run under node:test without a Lynx runtime.
import { create } from "@bufbuild/protobuf";
import {
  EntityKind,
  RelationshipKind,
  SortDirection,
  TaskFilterSchema,
  TaskPriority,
  TaskSortKey,
  TaskType,
  type EntityResult as ProtoEntityResult,
  type EntityVersion as ProtoEntityVersion,
  type Task as ProtoTask,
} from "@delino/protos/lane/v1/lane_pb";
import { LaneError } from "./adapter.ts";
import { toUI, type UI } from "./serializable.ts";

type EntityResult = UI<ProtoEntityResult>;
type EntityVersion = UI<ProtoEntityVersion>;
type Task = UI<ProtoTask>;

export const priorityOptions = [
  [TaskPriority.UNSPECIFIED, "None"],
  [TaskPriority.URGENT, "Urgent"],
  [TaskPriority.HIGH, "High"],
  [TaskPriority.NORMAL, "Normal"],
  [TaskPriority.LOW, "Low"],
] as const;
export const typeOptions = [
  [TaskType.UNSPECIFIED, "None"],
  [TaskType.BUG, "Bug"],
  [TaskType.FEATURE, "Feature"],
  [TaskType.IMPROVEMENT, "Improvement"],
  [TaskType.DOCUMENTATION, "Documentation"],
  [TaskType.MAINTENANCE, "Maintenance"],
] as const;
export const relationshipOptions = [
  [RelationshipKind.PARENT, "Parent"],
  [RelationshipKind.SUBTASKS, "Subtasks"],
  [RelationshipKind.DEPENDS_ON, "Depends on"],
  [RelationshipKind.BLOCKS, "Blocks"],
  [RelationshipKind.RELATED, "Related"],
  [RelationshipKind.DUPLICATE_OF, "Duplicate of"],
] as const;
export const sortKeyOptions = [
  [TaskSortKey.UNSPECIFIED, "Default order"],
  [TaskSortKey.PRIORITY, "Priority"],
  [TaskSortKey.TYPE, "Type"],
  [TaskSortKey.START_DATE, "Start date"],
  [TaskSortKey.DUE_DATE, "Due date"],
  [TaskSortKey.STORY_POINTS, "Story points"],
  [TaskSortKey.MILESTONE_NAME, "Milestone name"],
  [TaskSortKey.CYCLE_START_DATE, "Cycle start date"],
] as const;
export const sortDirectionOptions = [
  [SortDirection.ASCENDING, "Ascending"],
  [SortDirection.DESCENDING, "Descending"],
] as const;

export const MAX_LABELS = 100;
export const MAX_STORY_POINTS = 2147483647;
export const MAX_NAME = 100;
export const MAX_DESCRIPTION = 4000;

const enumLabel = <T extends number>(
  options: readonly (readonly [T, string])[],
  value: T,
) => options.find(([v]) => v === value)?.[1] ?? "None";
export const priorityLabel = (value: TaskPriority) =>
  enumLabel(priorityOptions, value);
export const typeLabel = (value: TaskType) => enumLabel(typeOptions, value);

// ---- Calendar dates and numbers -------------------------------------------

/** True for a real `YYYY-MM-DD` calendar date, leap years included. */
export function validDate(value: string): boolean {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (!match) return false;
  const [year, month, day] = [+match[1], +match[2], +match[3]];
  if (year < 1 || month < 1 || month > 12 || day < 1) return false;
  const leap = (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0;
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  return day <= days[month - 1];
}
/** Parses story-point text; undefined means unassigned, a string is a message. */
export function parsePoints(text: string): number | undefined | string {
  const value = text.trim();
  if (!value) return undefined;
  if (!/^\d+$/.test(value))
    return "Story points must be a whole number from 0 to 2,147,483,647.";
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed > MAX_STORY_POINTS)
    return "Story points must be a whole number from 0 to 2,147,483,647.";
  return parsed;
}
export const scalarLength = (value: string) => Array.from(value).length;
const DATE_HELP = "Use a real calendar date in YYYY-MM-DD form.";

export interface TaskFieldErrors {
  startDate?: string;
  dueDate?: string;
  storyPoints?: string;
  labels?: string;
}
export function validateTaskPlanning(
  task: Pick<Task, "startDate" | "dueDate" | "labelIds">,
  pointsText: string,
): TaskFieldErrors {
  const errors: TaskFieldErrors = {};
  const start = task.startDate ?? "";
  const due = task.dueDate ?? "";
  if (start && !validDate(start)) errors.startDate = DATE_HELP;
  if (due && !validDate(due)) errors.dueDate = DATE_HELP;
  if (!errors.startDate && !errors.dueDate && start && due && start > due)
    errors.startDate = "The start date must be on or before the due date.";
  const points = parsePoints(pointsText);
  if (typeof points === "string") errors.storyPoints = points;
  if (task.labelIds.length > MAX_LABELS)
    errors.labels = `A task can have at most ${MAX_LABELS} labels.`;
  return errors;
}
export const hasErrors = (errors: object) => Object.keys(errors).length > 0;

// ---- Error explanations ----------------------------------------------------

const normalize = (value: string | undefined, prefix: string) =>
  (value || "").replace(new RegExp(`^${prefix}`), "");
const violationText: Record<string, string> = {
  PARENT_CYCLE:
    "This parent would create a loop: a task cannot be below its own subtask, directly or indirectly. Choose a different parent or remove the link first.",
  DEPENDENCY_CYCLE:
    "These dependencies would create a loop: a task cannot wait on work that already depends on it. Remove one of the links.",
  DUPLICATE_CYCLE:
    "This duplicate link would create a loop of duplicates. Choose a different original task.",
  SELF_LINK: "A task cannot be linked to itself.",
  CROSS_REPOSITORY:
    "A label, milestone, cycle or linked task belongs to a different repository. Choose items from this task’s repository.",
  REFERENCE_NOT_FOUND:
    "A selected label, milestone, cycle or linked task no longer exists. Refresh the options and choose again.",
  DUPLICATE_NAME:
    "Another item of this kind in this repository already uses this name. Names are compared without regard to case.",
  LABEL_LIMIT: `A task can have at most ${MAX_LABELS} labels.`,
  DATE_ORDER: "The start date must be on or before the end or due date.",
  FIELD_RANGE: "A number is outside the allowed range.",
};
/** Violations that came from the server's accepted state during synchronization. */
export function violationFromReason(text: string): string | undefined {
  return /^CONSTRAINT_VIOLATION:([A-Z_]+)$/.exec(text.trim())?.[1];
}
/** Explains a rejected synchronization item (local reason text CONSTRAINT_VIOLATION:<VIOLATION>). */
export function describeItemError(reason: string): string {
  const violation = violationFromReason(reason);
  const text = violation ? violationText[violation] : undefined;
  return text
    ? `${text} Correct the item with an ordinary edit, then retry the blocked items.`
    : reason;
}
/** Explains an operation failure in text; the caller retains the draft. */
export function describeError(error: unknown): string {
  if (error instanceof LaneError) {
    const reason = normalize(error.detail.reason, "ERROR_REASON_");
    const violation =
      normalize(error.detail.violation, "CONSTRAINT_VIOLATION_") ||
      violationFromReason(error.detail.message) ||
      "";
    if (reason === "CONSTRAINT_VIOLATION" || violation) {
      const text = violationText[violation];
      if (text) return `${text} Your changes are not saved.`;
    }
    if (reason === "CONFLICT_READ_ONLY")
      return "A linked task has an unresolved conflict, so nothing was changed. Resolve that conflict first.";
    if (reason === "ONLINE_REQUIRED")
      return `${error.message} Reconnect, then retry explicitly.`;
  }
  return error instanceof Error
    ? error.message
    : "The operation failed. Retry explicitly.";
}

// ---- Planning items --------------------------------------------------------

export enum PlanningKind {
  Label = "label",
  Milestone = "milestone",
  Cycle = "cycle",
}
export const planningKinds = [
  [PlanningKind.Label, "Labels", "label", EntityKind.LABEL],
  [PlanningKind.Milestone, "Milestones", "milestone", EntityKind.MILESTONE],
  [PlanningKind.Cycle, "Cycles", "cycle", EntityKind.CYCLE],
] as const;
export interface PlanningItem {
  id: string;
  versionId: string;
  kind: PlanningKind;
  repositoryId: string;
  name: string;
  description: string;
  color: string;
  targetDate?: string;
  completed: boolean;
  startDate: string;
  endDate: string;
  conflicted: boolean;
}
export function planningItemFromVersion(
  version: EntityVersion | undefined,
  conflicted = false,
): PlanningItem | undefined {
  if (!version || version.deleted) return undefined;
  const body = version.body;
  const base = {
    id: version.entityId,
    versionId: version.versionId,
    conflicted,
    description: "",
    color: "",
    completed: false,
    startDate: "",
    endDate: "",
  };
  if (body.case === "label")
    return {
      ...base,
      kind: PlanningKind.Label,
      repositoryId: body.value.repositoryId,
      name: body.value.name ?? "",
      description: body.value.description ?? "",
      color: body.value.color ?? "",
    };
  if (body.case === "milestone")
    return {
      ...base,
      kind: PlanningKind.Milestone,
      repositoryId: body.value.repositoryId,
      name: body.value.name ?? "",
      description: body.value.description ?? "",
      targetDate: body.value.targetDate,
      completed: body.value.completed ?? false,
    };
  if (body.case === "cycle")
    return {
      ...base,
      kind: PlanningKind.Cycle,
      repositoryId: body.value.repositoryId,
      name: body.value.name ?? "",
      description: body.value.description ?? "",
      startDate: body.value.startDate ?? "",
      endDate: body.value.endDate ?? "",
    };
  return undefined;
}
export function planningItemOf(result: EntityResult): PlanningItem | undefined {
  const heads = new Set(
    [result.version, ...result.alternatives]
      .filter((v) => v)
      .map((v) => v!.versionId),
  );
  return planningItemFromVersion(result.version, heads.size > 1);
}
export function planningItems(results: EntityResult[]): PlanningItem[] {
  return results
    .map(planningItemOf)
    .filter((item): item is PlanningItem => !!item)
    .sort(
      (a, b) =>
        a.name.toLowerCase().localeCompare(b.name.toLowerCase()) ||
        a.id.localeCompare(b.id),
    );
}
export const itemsOf = (
  items: PlanningItem[],
  kind: PlanningKind,
  repositoryId?: string,
) =>
  items.filter(
    (i) =>
      i.kind === kind && (!repositoryId || i.repositoryId === repositoryId),
  );
export interface PlanningInput {
  name: string;
  description: string;
  color: string;
  targetDate: string;
  completed: boolean;
  startDate: string;
  endDate: string;
}
export const blankPlanningInput = (): PlanningInput => ({
  name: "",
  description: "",
  color: "#075AFF",
  targetDate: "",
  completed: false,
  startDate: "",
  endDate: "",
});
export function planningInputOf(item: PlanningItem): PlanningInput {
  return {
    name: item.name,
    description: item.description,
    color: item.color || "#075AFF",
    targetDate: item.targetDate ?? "",
    completed: item.completed,
    startDate: item.startDate,
    endDate: item.endDate,
  };
}
export type PlanningErrors = Partial<Record<keyof PlanningInput, string>>;
export function validatePlanningInput(
  kind: PlanningKind,
  input: PlanningInput,
  siblings: PlanningItem[],
  selfId?: string,
): PlanningErrors {
  const errors: PlanningErrors = {};
  const name = input.name.trim();
  if (!name) errors.name = "Enter a name.";
  else if (scalarLength(name) > MAX_NAME)
    errors.name = `A name can have at most ${MAX_NAME} characters.`;
  else if (
    siblings.some(
      (s) =>
        s.id !== selfId &&
        s.kind === kind &&
        s.name.trim().toLowerCase() === name.toLowerCase(),
    )
  )
    errors.name =
      "Another item of this kind in this repository already uses this name. Names are compared without regard to case.";
  if (scalarLength(input.description) > MAX_DESCRIPTION)
    errors.description = `A description can have at most ${MAX_DESCRIPTION} characters.`;
  if (kind === PlanningKind.Label && !/^#[0-9A-Fa-f]{6}$/.test(input.color))
    errors.color = "Use a color in #RRGGBB form, for example #075AFF.";
  if (kind === PlanningKind.Milestone && input.targetDate) {
    if (!validDate(input.targetDate)) errors.targetDate = DATE_HELP;
  }
  if (kind === PlanningKind.Cycle) {
    if (!input.startDate) errors.startDate = "Enter a start date.";
    else if (!validDate(input.startDate)) errors.startDate = DATE_HELP;
    if (!input.endDate) errors.endDate = "Enter an end date.";
    else if (!validDate(input.endDate)) errors.endDate = DATE_HELP;
    if (!errors.startDate && !errors.endDate && input.startDate > input.endDate)
      errors.endDate = "The end date must be on or after the start date.";
  }
  return errors;
}
export function planningBody(
  kind: PlanningKind,
  repositoryId: string,
  input: PlanningInput,
) {
  const common = {
    repositoryId,
    name: input.name.trim(),
    description: input.description,
  };
  switch (kind) {
    case PlanningKind.Label:
      return {
        body: {
          case: "label" as const,
          value: { ...common, color: input.color.toUpperCase() },
        },
      };
    case PlanningKind.Milestone:
      return {
        body: {
          case: "milestone" as const,
          value: {
            ...common,
            targetDate: input.targetDate || undefined,
            completed: input.completed,
          },
        },
      };
    case PlanningKind.Cycle:
      return {
        body: {
          case: "cycle" as const,
          value: {
            ...common,
            startDate: input.startDate,
            endDate: input.endDate,
          },
        },
      };
  }
}

export interface Change {
  field: string;
  before: string;
  after: string;
}
const textOf = (value: unknown) =>
  value === undefined || value === "" ? "(none)" : String(value);
export function planningItemChanges(
  before: EntityVersion | undefined,
  after: EntityVersion,
): Change[] {
  const next = planningItemFromVersion(after);
  if (!next) return [];
  const old = planningItemFromVersion(before);
  const fields: [keyof PlanningItem, string][] = [
    ["name", "Name"],
    ["description", "Description"],
    ["color", "Color"],
    ["targetDate", "Target date"],
    ["completed", "Completed"],
    ["startDate", "Start date"],
    ["endDate", "End date"],
  ];
  const wanted = fields.filter(([key]) =>
    next.kind === PlanningKind.Label
      ? ["name", "description", "color"].includes(key)
      : next.kind === PlanningKind.Milestone
        ? ["name", "description", "targetDate", "completed"].includes(key)
        : ["name", "description", "startDate", "endDate"].includes(key),
  );
  return wanted
    .filter(([key]) => old === undefined || old[key] !== next[key])
    .map(([key, field]) => ({
      field,
      before: old
        ? key === "completed"
          ? old.completed
            ? "Yes"
            : "No"
          : textOf(old[key])
        : "(new item)",
      after:
        key === "completed"
          ? next.completed
            ? "Yes"
            : "No"
          : textOf(next[key]),
    }));
}

// ---- Names and task metadata -----------------------------------------------

export interface Lookup {
  item(id: string): PlanningItem | undefined;
  task(id: string): string | undefined;
  repository(id: string): string;
}
export function lookupOf(
  items: PlanningItem[],
  tasks: Map<string, string>,
  repositories: Map<string, string>,
): Lookup {
  const byId = new Map(items.map((i) => [i.id, i]));
  return {
    item: (id) => byId.get(id),
    task: (id) => tasks.get(id),
    repository: (id) => repositories.get(id) ?? "Repository",
  };
}
export const qualifiedName = (
  item: PlanningItem,
  lookup: Lookup,
  qualify: boolean,
) =>
  qualify
    ? `${lookup.repository(item.repositoryId)} / ${item.name}`
    : item.name;

export interface TaskMetadata {
  summary: string[];
  labels: string[];
  relationships: string[];
}
/** Compact metadata for list rows. Repository-owned values qualify in all-repository lists. */
export function taskMetadata(
  task: Task,
  lookup: Lookup,
  qualify: boolean,
): TaskMetadata {
  const summary: string[] = [];
  if (task.priority !== TaskPriority.UNSPECIFIED)
    summary.push(`Priority ${priorityLabel(task.priority)}`);
  if (task.type !== TaskType.UNSPECIFIED)
    summary.push(`Type ${typeLabel(task.type)}`);
  if (task.startDate) summary.push(`Starts ${task.startDate}`);
  if (task.dueDate) summary.push(`Due ${task.dueDate}`);
  if (task.storyPoints !== undefined)
    summary.push(
      `${task.storyPoints} ${task.storyPoints === 1 ? "point" : "points"}`,
    );
  const milestone = task.milestoneId
    ? lookup.item(task.milestoneId)
    : undefined;
  if (task.milestoneId)
    summary.push(
      `Milestone ${milestone ? qualifiedName(milestone, lookup, qualify) : "(unavailable)"}`,
    );
  const cycle = task.cycleId ? lookup.item(task.cycleId) : undefined;
  if (task.cycleId)
    summary.push(
      `Cycle ${cycle ? qualifiedName(cycle, lookup, qualify) : "(unavailable)"}`,
    );
  const labels = task.labelIds.map((id) => {
    const label = lookup.item(id);
    return label
      ? qualifiedName(label, lookup, qualify)
      : "(unavailable label)";
  });
  const relationships: string[] = [];
  if (task.parentTaskId) relationships.push("Has parent");
  if (task.dependsOnTaskIds.length)
    relationships.push(`Depends on ${task.dependsOnTaskIds.length}`);
  if (task.relatedTaskIds.length)
    relationships.push(`Related ${task.relatedTaskIds.length}`);
  if (task.duplicateOfTaskId) relationships.push("Duplicate");
  return { summary, labels, relationships };
}

// ---- Task history ----------------------------------------------------------

export const planningHistoryFields = [
  "priority",
  "type",
  "labelIds",
  "startDate",
  "dueDate",
  "storyPoints",
  "milestoneId",
  "cycleId",
  "parentTaskId",
  "dependsOnTaskIds",
  "relatedTaskIds",
  "duplicateOfTaskId",
] as const;
const planningFieldNames: Record<
  (typeof planningHistoryFields)[number],
  string
> = {
  priority: "Priority",
  type: "Type",
  labelIds: "Labels",
  startDate: "Start date",
  dueDate: "Due date",
  storyPoints: "Story points",
  milestoneId: "Milestone",
  cycleId: "Cycle",
  parentTaskId: "Parent",
  dependsOnTaskIds: "Depends on",
  relatedTaskIds: "Related",
  duplicateOfTaskId: "Duplicate of",
};
export function planningTaskChanges(
  old: Task | undefined,
  next: Task,
  lookup: Lookup,
): Change[] {
  const itemName = (id: string) => {
    const item = lookup.item(id);
    return item ? item.name : "(deleted item)";
  };
  const taskTitle = (id: string) => lookup.task(id) ?? "(unavailable task)";
  const format = (
    task: Task | undefined,
    field: (typeof planningHistoryFields)[number],
  ) => {
    if (!task) return "(none)";
    switch (field) {
      case "priority":
        return priorityLabel(task.priority);
      case "type":
        return typeLabel(task.type);
      case "labelIds":
        return task.labelIds.length
          ? task.labelIds.map(itemName).join(", ")
          : "(none)";
      case "milestoneId":
      case "cycleId":
        return task[field] ? itemName(task[field]!) : "(none)";
      case "parentTaskId":
      case "duplicateOfTaskId":
        return task[field] ? taskTitle(task[field]!) : "(none)";
      case "dependsOnTaskIds":
      case "relatedTaskIds":
        return task[field].length
          ? task[field].map(taskTitle).join(", ")
          : "(none)";
      case "storyPoints":
        return task.storyPoints === undefined
          ? "(none)"
          : String(task.storyPoints);
      default:
        return textOf(task[field]);
    }
  };
  const same = (a: unknown, b: unknown) =>
    JSON.stringify(a) === JSON.stringify(b);
  return planningHistoryFields
    .filter((field) => {
      const before = old
        ? old[field]
        : field.endsWith("Ids")
          ? []
          : field === "priority" || field === "type"
            ? 0
            : undefined;
      return !same(before, next[field]);
    })
    .map((field) => ({
      field: planningFieldNames[field],
      before: format(old, field),
      after: format(next, field),
    }));
}

// ---- Relationships ---------------------------------------------------------

export interface Relations {
  subtasks: string[];
  blocks: string[];
  baseSubtasks: string[];
  baseBlocks: string[];
}
export interface TaskRef {
  id: string;
  title: string;
  task: Task;
}
const sorted = (values: Iterable<string>) => [...new Set(values)].sort();
/** Subtasks and Blocks are derived inverse views of other tasks' stored fields. */
export function deriveRelations(taskId: string, tasks: TaskRef[]): Relations {
  const subtasks = sorted(
    tasks.filter((t) => t.task.parentTaskId === taskId).map((t) => t.id),
  );
  const blocks = sorted(
    tasks
      .filter((t) => t.task.dependsOnTaskIds.includes(taskId))
      .map((t) => t.id),
  );
  return { subtasks, blocks, baseSubtasks: subtasks, baseBlocks: blocks };
}
/** Related is symmetric: show the union of both stored sides. */
export function unionRelated(taskId: string, own: string[], tasks: TaskRef[]) {
  return sorted([
    ...own,
    ...tasks
      .filter((t) => t.task.relatedTaskIds.includes(taskId))
      .map((t) => t.id),
  ]);
}
export function relationshipEdits(relations?: Relations) {
  if (!relations) return undefined;
  const added = (next: string[], base: string[]) =>
    next.filter((id) => !base.includes(id));
  const edits = {
    addSubtaskIds: added(relations.subtasks, relations.baseSubtasks),
    removeSubtaskIds: added(relations.baseSubtasks, relations.subtasks),
    addBlockedTaskIds: added(relations.blocks, relations.baseBlocks),
    removeBlockedTaskIds: added(relations.baseBlocks, relations.blocks),
  };
  return Object.values(edits).some((v) => v.length) ? edits : undefined;
}
export const toggled = <T>(values: readonly T[], value: T): T[] =>
  values.includes(value)
    ? values.filter((v) => v !== value)
    : [...values, value];

// ---- List filters ----------------------------------------------------------

export interface FilterState {
  priorities: TaskPriority[];
  unassignedPriority: boolean;
  types: TaskType[];
  unassignedType: boolean;
  labelIds: string[];
  unlabeled: boolean;
  startFrom: string;
  startTo: string;
  startUnassigned: boolean;
  dueFrom: string;
  dueTo: string;
  dueUnassigned: boolean;
  pointsMin: string;
  pointsMax: string;
  pointsUnassigned: boolean;
  milestoneIds: string[];
  noMilestone: boolean;
  cycleIds: string[];
  noCycle: boolean;
  relationships: RelationshipKind[];
  noRelationships: boolean;
}
export const emptyFilter = (): FilterState => ({
  priorities: [],
  unassignedPriority: false,
  types: [],
  unassignedType: false,
  labelIds: [],
  unlabeled: false,
  startFrom: "",
  startTo: "",
  startUnassigned: false,
  dueFrom: "",
  dueTo: "",
  dueUnassigned: false,
  pointsMin: "",
  pointsMax: "",
  pointsUnassigned: false,
  milestoneIds: [],
  noMilestone: false,
  cycleIds: [],
  noCycle: false,
  relationships: [],
  noRelationships: false,
});
export type FilterErrors = Partial<Record<"start" | "due" | "points", string>>;
function dateRangeError(from: string, to: string): string | undefined {
  if ((from && !validDate(from)) || (to && !validDate(to))) return DATE_HELP;
  if (from && to && from > to)
    return "The first date must not be after the second.";
  return undefined;
}
function pointsError(min: string, max: string): string | undefined {
  const low = parsePoints(min);
  const high = parsePoints(max);
  if (typeof low === "string" || typeof high === "string")
    return "Use whole numbers from 0 to 2,147,483,647.";
  if (low !== undefined && high !== undefined && low > high)
    return "The minimum must not be above the maximum.";
  return undefined;
}
export function filterErrors(f: FilterState): FilterErrors {
  const errors: FilterErrors = {};
  const start = dateRangeError(f.startFrom, f.startTo);
  const due = dateRangeError(f.dueFrom, f.dueTo);
  const points = pointsError(f.pointsMin, f.pointsMax);
  if (start) errors.start = start;
  if (due) errors.due = due;
  if (points) errors.points = points;
  return errors;
}
/** Number of filter fields with a condition, shown on the collapsed Filters control. */
export function activeFilterCount(f: FilterState): number {
  return [
    f.priorities.length > 0 || f.unassignedPriority,
    f.types.length > 0 || f.unassignedType,
    f.labelIds.length > 0 || f.unlabeled,
    !!f.startFrom || !!f.startTo || f.startUnassigned,
    !!f.dueFrom || !!f.dueTo || f.dueUnassigned,
    !!f.pointsMin || !!f.pointsMax || f.pointsUnassigned,
    f.milestoneIds.length > 0 || f.noMilestone,
    f.cycleIds.length > 0 || f.noCycle,
    f.relationships.length > 0 || f.noRelationships,
  ].filter(Boolean).length;
}
/** Builds the typed TaskFilter; callers must not send it while filterErrors is non-empty. */
export function toTaskFilter(f: FilterState) {
  if (activeFilterCount(f) === 0) return undefined;
  const range = (from: string, to: string, unassigned: boolean) =>
    from || to || unassigned
      ? {
          from: from || undefined,
          to: to || undefined,
          includeUnassigned: unassigned,
        }
      : undefined;
  const points =
    f.pointsMin || f.pointsMax || f.pointsUnassigned
      ? {
          min: f.pointsMin ? Number(f.pointsMin) : undefined,
          max: f.pointsMax ? Number(f.pointsMax) : undefined,
          includeUnassigned: f.pointsUnassigned,
        }
      : undefined;
  return toUI(
    create(TaskFilterSchema, {
      priorities: f.priorities,
      includeUnassignedPriority: f.unassignedPriority,
      types: f.types,
      includeUnassignedType: f.unassignedType,
      labelIds: f.labelIds,
      includeUnlabeled: f.unlabeled,
      startDate: range(f.startFrom, f.startTo, f.startUnassigned),
      dueDate: range(f.dueFrom, f.dueTo, f.dueUnassigned),
      storyPoints: points,
      milestoneIds: f.milestoneIds,
      includeNoMilestone: f.noMilestone,
      cycleIds: f.cycleIds,
      includeNoCycle: f.noCycle,
      relationships: f.relationships,
      includeNoRelationships: f.noRelationships,
    }),
  );
}
/** Drops selections of repository-owned values that are not offered in the current scope. */
export function pruneFilter(
  f: FilterState,
  items: PlanningItem[],
): FilterState {
  const keep = (ids: string[]) =>
    ids.filter((id) => items.some((i) => i.id === id));
  const next = {
    ...f,
    labelIds: keep(f.labelIds),
    milestoneIds: keep(f.milestoneIds),
    cycleIds: keep(f.cycleIds),
  };
  return next.labelIds.length === f.labelIds.length &&
    next.milestoneIds.length === f.milestoneIds.length &&
    next.cycleIds.length === f.cycleIds.length
    ? f
    : next;
}
