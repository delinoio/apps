// SPDX-License-Identifier: Apache-2.0
import { toUI, fromUI, type UI } from "./serializable.ts";
import { create } from "@bufbuild/protobuf";
import {
  planningTaskChanges,
  type Lookup,
  type Relations,
} from "./planning.ts";
import {
  TaskSchema,
  Progress,
  Proposal,
  ResultKind,
  type EntityResult as ProtoEntityResult,
  type EntityVersion as ProtoEntityVersion,
  type Task as ProtoTask,
} from "@repo/protos/lane/v1/lane_pb";
export const progressOptions = [
  [Progress.TODO, "To do"],
  [Progress.IN_PROGRESS, "In progress"],
  [Progress.BLOCKED, "Blocked"],
  [Progress.DONE, "Done"],
  [Progress.CANCELED, "Canceled"],
] as const;
export const proposalOptions = [
  [Proposal.REGULAR, "Regular"],
  [Proposal.PENDING, "Pending"],
  [Proposal.ACCEPTED, "Accepted"],
  [Proposal.REJECTED, "Rejected"],
] as const;
export function taskOf(version?: EntityVersion): Task | undefined {
  return version?.body.case === "task" ? version.body.value : undefined;
}
export function repositoryName(version?: EntityVersion): string {
  return version?.body.case === "repository"
    ? version.body.value.displayName || version.body.value.normalizedUrl
    : version?.entityId || "Repository";
}
export function heads(result: EntityResult): EntityVersion[] {
  return [
    ...new Map(
      [result.version, ...result.alternatives]
        .filter((v): v is EntityVersion => !!v)
        .map((v) => [v.versionId, v]),
    ).values(),
  ];
}
export function conflicted(result?: EntityResult): boolean {
  return (
    !!result &&
    (result.result === ResultKind.CONFLICT || heads(result).length > 1)
  );
}
export function blankTask(repositoryId = ""): Task {
  return toUI(
    create(TaskSchema, {
      repositoryId,
      title: "",
      description: "",
      progress: Progress.TODO,
      proposal: Proposal.REGULAR,
    }),
  );
}
export interface Draft {
  id: string;
  base?: string;
  task: Task;
  // Saved snapshot: detects a move (repository change) and restores the
  // repository-owned values when the user selects the original repository again.
  baseTask?: Task;
  // Derived Subtasks/Blocks edits, loaded with the same-repository task options.
  relations?: Relations;
  dirty: boolean;
  external: boolean;
  deleted: boolean;
}
export function draftFor(result: EntityResult): Draft {
  if (!result.version || !taskOf(result.version))
    throw new Error("Task is unavailable.");
  return {
    id: result.version.entityId,
    base: result.version.versionId,
    task: { ...taskOf(result.version)! },
    baseTask: { ...taskOf(result.version)! },
    dirty: false,
    external: false,
    deleted: false,
  };
}
export function receiveExternal(draft: Draft, result: EntityResult): Draft {
  if (result.result === ResultKind.DELETED || result.version?.deleted)
    return { ...draft, external: true, deleted: true };
  if (result.version?.versionId === draft.base) return draft;
  // Explicit editor lifetime: incoming reads never replace current-screen input.
  return { ...draft, external: true };
}
export type HistoryBaseline =
  | { kind: "creation" }
  | { kind: "parent"; version: EntityVersion }
  | { kind: "pending" }
  | { kind: "merge" };
/**
 * Selects the snapshot a history entry is compared with. The list is chronological
 * and includes retained alternatives, so neighbouring positions say nothing about
 * ancestry: only the entry's declared parent is a valid baseline.
 */
export function historyBaseline(
  history: readonly EntityVersion[],
  version: EntityVersion,
): HistoryBaseline {
  const parents = version.parentVersionIds || [];
  if (parents.length === 0) return { kind: "creation" };
  // A resolution has several parents and therefore no single before snapshot.
  if (parents.length > 1) return { kind: "merge" };
  const parent = history.find((entry) => entry.versionId === parents[0]);
  // The parent may be on a page that is not loaded yet; never guess a baseline.
  return parent ? { kind: "parent", version: parent } : { kind: "pending" };
}
/** Field changes of one history entry against its declared parent, if comparable. */
export function historyEntryChanges(
  history: readonly EntityVersion[],
  version: EntityVersion,
  lookup?: Lookup,
): { field: string; before: string; after: string }[] {
  const baseline = historyBaseline(history, version);
  if (baseline.kind === "creation")
    return historyChanges(undefined, version, lookup);
  if (baseline.kind === "parent")
    return historyChanges(baseline.version, version, lookup);
  return [];
}
export function historyChanges(
  before: EntityVersion | undefined,
  after: EntityVersion,
  lookup?: Lookup,
): { field: string; before: string; after: string }[] {
  const old = taskOf(before);
  const next = taskOf(after);
  if (!next) return [];
  const label = (field: string, value: unknown) =>
    field === "progress"
      ? progressOptions.find(([v]) => v === value)?.[1] || String(value)
      : field === "proposal"
        ? proposalOptions.find(([v]) => v === value)?.[1] || String(value)
        : value === undefined
          ? "(absent)"
          : String(value);
  const core = (
    ["title", "description", "progress", "proposal", "repositoryId"] as const
  )
    .filter((k) => old?.[k] !== next[k])
    .map((k) => ({
      field: k as string,
      before: label(k, old?.[k]),
      after: label(k, next[k]),
    }));
  // Planning fields and relationships are task fields too; names come from the
  // current lookups and never from copies stored in this history.
  return lookup ? [...core, ...planningTaskChanges(old, next, lookup)] : core;
}

type EntityResult = UI<ProtoEntityResult>;

type EntityVersion = UI<ProtoEntityVersion>;

type Task = UI<ProtoTask>;
export function createUITask(value: UI<ProtoTask>): UI<ProtoTask> {
  return toUI(fromUI(TaskSchema, value));
}
