// SPDX-License-Identifier: Apache-2.0
import type { UI } from "./serializable";
import type { Task as ProtoTask } from "@delino/protos/lane/v1/lane_pb";
import { Button, Choices, Field, MultiChoices, Notice } from "./components";
import type { Draft } from "./model";
import {
  MAX_LABELS,
  PlanningKind,
  itemsOf,
  priorityOptions,
  toggled,
  typeOptions,
  type TaskFieldErrors,
} from "./planning";
import type { PlanningState } from "./usePlanning";

type Task = UI<ProtoTask>;
export function TaskProperties({
  draft,
  edit,
  disabled,
  planning,
  pointsText,
  setPointsText,
  errors,
  moved,
}: {
  draft: Draft;
  edit: (patch: Partial<Task>) => void;
  disabled: boolean;
  planning: PlanningState;
  pointsText: string;
  setPointsText: (value: string) => void;
  errors: TaskFieldErrors;
  moved: boolean;
}) {
  const task = draft.task;
  const repositoryId = task.repositoryId;
  const ownedDisabled = disabled || !repositoryId || moved;
  const labels = itemsOf(planning.items, PlanningKind.Label, repositoryId);
  const milestones = itemsOf(
    planning.items,
    PlanningKind.Milestone,
    repositoryId,
  );
  const cycles = itemsOf(planning.items, PlanningKind.Cycle, repositoryId);
  const unavailable = (ids: string[], known: { id: string }[], noun: string) =>
    ids
      .filter((id) => !known.some((k) => k.id === id))
      .map((id) => ({ id, title: `(unavailable ${noun})` }));
  const labelOptions = [
    ...labels.map((l) => ({ id: l.id, title: l.name, swatch: l.color })),
    ...unavailable(task.labelIds, labels, "label"),
  ];
  const single = (
    id: string | undefined,
    items: { id: string; name: string; suffix?: string }[],
    noun: string,
  ): (readonly [string, string])[] => [
    ["", "None"],
    ...items.map(
      (i) => [i.id, i.suffix ? `${i.name} (${i.suffix})` : i.name] as const,
    ),
    ...(id && !items.some((i) => i.id === id)
      ? [[id, `(unavailable ${noun})`] as const]
      : []),
  ];
  return (
    <view className="planning-section">
      <text className="heading" accessibility-heading>
        Properties
      </text>
      <Choices
        exclusive
        label="Priority"
        value={task.priority}
        options={priorityOptions}
        onChange={(priority) => edit({ priority })}
        disabled={disabled}
      />
      <Choices
        exclusive
        label="Type"
        value={task.type}
        options={typeOptions}
        onChange={(type) => edit({ type })}
        disabled={disabled}
      />
      <view className="field-grid">
        <Field
          label="Start date (YYYY-MM-DD)"
          value={task.startDate ?? ""}
          onChange={(value) => edit({ startDate: value.trim() || undefined })}
          disabled={disabled}
          error={errors.startDate}
        />
        <Field
          label="Due date (YYYY-MM-DD)"
          value={task.dueDate ?? ""}
          onChange={(value) => edit({ dueDate: value.trim() || undefined })}
          disabled={disabled}
          error={errors.dueDate}
        />
        <Field
          label="Story points (whole number)"
          value={pointsText}
          onChange={setPointsText}
          disabled={disabled}
          error={errors.storyPoints}
          help="Leave empty for unassigned. 0 is a value."
        />
      </view>
      {moved && (
        <Notice>
          Moving this task clears its labels, milestone, cycle and
          relationships; priority, type, dates and story points are kept. Save
          the move first, then assign labels, a milestone, a cycle and links in
          the new repository.
        </Notice>
      )}
      {!repositoryId && !moved && (
        <Notice>
          Choose an existing repository to assign labels, a milestone or a
          cycle. A repository URL registers a new repository when the task is
          created.
        </Notice>
      )}
      {repositoryId && planning.status === "loading" && (
        <text className="muted">Loading labels, milestones and cycles…</text>
      )}
      {repositoryId && planning.status === "error" && (
        <Notice>
          Could not load labels, milestones and cycles. {planning.error}{" "}
          <Button onClick={planning.retry}>Retry loading options</Button>
        </Notice>
      )}
      <MultiChoices
        label="Labels"
        selected={task.labelIds}
        options={labelOptions}
        disabled={ownedDisabled}
        onToggle={(id) => {
          if (!task.labelIds.includes(id) && task.labelIds.length >= MAX_LABELS)
            return;
          edit({ labelIds: toggled(task.labelIds, id).sort() });
        }}
      />
      <text className="muted">
        {task.labelIds.length} of {MAX_LABELS} labels selected.
        {task.labelIds.length >= MAX_LABELS
          ? " The limit is reached; remove a label to add another."
          : ""}
        {repositoryId &&
        planning.status === "ready" &&
        labels.length === 0 &&
        !moved
          ? " This repository has no labels yet; create them in Repositories."
          : ""}
      </text>
      {errors.labels && <text className="validation">{errors.labels}</text>}
      <Choices
        exclusive
        label="Milestone"
        value={task.milestoneId ?? ""}
        options={single(task.milestoneId, milestones, "milestone")}
        onChange={(id) => edit({ milestoneId: id || undefined })}
        disabled={ownedDisabled}
      />
      <Choices
        exclusive
        label="Cycle"
        value={task.cycleId ?? ""}
        options={single(
          task.cycleId,
          cycles.map((c) => ({
            id: c.id,
            name: c.name,
            suffix: `${c.startDate} to ${c.endDate}`,
          })),
          "cycle",
        )}
        onChange={(id) => edit({ cycleId: id || undefined })}
        disabled={ownedDisabled}
      />
    </view>
  );
}
