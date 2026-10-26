// SPDX-License-Identifier: Apache-2.0
import { toUI, fromUI, type UI } from "./serializable.ts";
import { create } from "@bufbuild/protobuf";
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
export function historyChanges(
  before: EntityVersion | undefined,
  after: EntityVersion,
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
  return (
    ["title", "description", "progress", "proposal", "repositoryId"] as const
  )
    .filter((k) => old?.[k] !== next[k])
    .map((k) => ({
      field: k,
      before: label(k, old?.[k]),
      after: label(k, next[k]),
    }));
}

type EntityResult = UI<ProtoEntityResult>;

type EntityVersion = UI<ProtoEntityVersion>;

type Task = UI<ProtoTask>;
export function createUITask(value: UI<ProtoTask>): UI<ProtoTask> {
  return toUI(fromUI(TaskSchema, value));
}
