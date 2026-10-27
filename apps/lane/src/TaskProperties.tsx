// SPDX-License-Identifier: Apache-2.0
import { useState } from "@lynx-js/react";
import type { UI } from "./serializable";
import type { Task as ProtoTask } from "@repo/protos/lane/v1/lane_pb";
import { Button, Field, Notice } from "./components";
import { TaskLabelSelect, TaskSelect } from "./TaskControls";
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
const propertyFields = [
  ["priority", "Priority"],
  ["type", "Type"],
  ["startDate", "Start date"],
  ["dueDate", "Due date"],
  ["storyPoints", "Story points"],
  ["labels", "Labels"],
  ["milestone", "Milestone"],
  ["cycle", "Cycle"],
] as const;
type PropertyField = (typeof propertyFields)[number][0];
export function TaskProperties({
  draft,
  edit,
  disabled,
  planning,
  pointsText,
  setPointsText,
  errors,
  moved,
  compact = false,
  openControl,
  setOpenControl,
}: {
  draft: Draft;
  edit: (patch: Partial<Task>) => void;
  disabled: boolean;
  planning: PlanningState;
  pointsText: string;
  setPointsText: (value: string) => void;
  errors: TaskFieldErrors;
  moved: boolean;
  compact?: boolean;
  openControl: string | undefined;
  setOpenControl: (control: string | undefined) => void;
}) {
  const [activeField, setActiveField] = useState<PropertyField>();
  const [menuIndex, setMenuIndex] = useState(0);
  const [triggerId] = useState(
    () => `task-properties-${Math.random().toString(36).slice(2)}`,
  );
  const focusTrigger = () => {
    "background only";
    lynx
      .createSelectorQuery()
      .select(`#${triggerId}`)
      .invoke({ method: "setFocus", params: { focus: true } })
      .exec();
  };
  const menuOpen = openControl === "property-fields";
  const closeMenu = () => {
    setOpenControl(undefined);
    focusTrigger();
  };
  const menuKey = (key: string) => {
    if (key === "Escape" && menuOpen) {
      closeMenu();
      return true;
    }
    if ((key === "ArrowDown" || key === "ArrowUp") && menuOpen) {
      setMenuIndex(
        (menuIndex + (key === "ArrowDown" ? 1 : propertyFields.length - 1)) %
          propertyFields.length,
      );
      return true;
    }
    return false;
  };
  const showField = (field: PropertyField) => {
    if (disabled) return;
    setActiveField(field);
    setOpenControl(
      ["priority", "type", "milestone", "cycle", "labels"].includes(field)
        ? field
        : undefined,
    );
  };
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
  if (compact) {
    const selected: {
      field: PropertyField;
      label: string;
      value: string;
      id?: string;
    }[] = [];
    if (task.priority)
      selected.push({
        field: "priority",
        label: "Priority",
        value:
          priorityOptions.find(([value]) => value === task.priority)?.[1] ??
          "Unavailable",
      });
    if (task.type)
      selected.push({
        field: "type",
        label: "Type",
        value:
          typeOptions.find(([value]) => value === task.type)?.[1] ??
          "Unavailable",
      });
    if (task.startDate)
      selected.push({
        field: "startDate",
        label: "Start",
        value: task.startDate,
      });
    if (task.dueDate)
      selected.push({ field: "dueDate", label: "Due", value: task.dueDate });
    if (pointsText !== "")
      selected.push({
        field: "storyPoints",
        label: "Points",
        value: pointsText,
      });
    for (const id of task.labelIds)
      selected.push({
        field: "labels",
        label: "Label",
        value:
          labelOptions.find((option) => option.id === id)?.title ??
          "(unavailable label)",
        id,
      });
    if (task.milestoneId)
      selected.push({
        field: "milestone",
        label: "Milestone",
        value:
          single(task.milestoneId, milestones, "milestone").find(
            ([id]) => id === task.milestoneId,
          )?.[1] ?? "(unavailable milestone)",
      });
    if (task.cycleId)
      selected.push({
        field: "cycle",
        label: "Cycle",
        value:
          single(
            task.cycleId,
            cycles.map((cycle) => ({
              id: cycle.id,
              name: cycle.name,
              suffix: `${cycle.startDate} to ${cycle.endDate}`,
            })),
            "cycle",
          ).find(([id]) => id === task.cycleId)?.[1] ?? "(unavailable cycle)",
      });
    return (
      <view className="planning-section task-properties compact-planning">
        <view className="row between wrap">
          <text className="heading" accessibility-heading>
            Properties
          </text>
          <Button
            elementId={triggerId}
            label="Add or edit property"
            expanded={menuOpen}
            disabled={disabled}
            onKeyDown={menuKey}
            onClick={() => {
              if (menuOpen) closeMenu();
              else {
                setMenuIndex(0);
                setOpenControl("property-fields");
              }
            }}
          >
            +
          </Button>
        </view>
        {menuOpen && (
          <view className="task-select-options">
            {propertyFields.map(([field, label], index) => (
              <Button
                key={field}
                focusOnShow={index === menuIndex}
                onKeyDown={menuKey}
                onClick={() => showField(field)}
              >
                {label}
              </Button>
            ))}
          </view>
        )}
        {selected.length ? (
          <view className="row wrap property-chips">
            {selected.map(({ field, label, value, id }) => (
              <Button
                key={`${field}-${id ?? ""}`}
                label={`Edit ${label}: ${value}`}
                disabled={disabled}
                onClick={() => showField(field)}
              >
                {label}: {value}
              </Button>
            ))}
          </view>
        ) : (
          <text className="muted">No properties selected.</text>
        )}
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
        {errors.startDate && activeField !== "startDate" && (
          <text className="validation">Start date: {errors.startDate}</text>
        )}
        {errors.dueDate && activeField !== "dueDate" && (
          <text className="validation">Due date: {errors.dueDate}</text>
        )}
        {errors.storyPoints && activeField !== "storyPoints" && (
          <text className="validation">Story points: {errors.storyPoints}</text>
        )}
        {errors.labels && activeField !== "labels" && (
          <text className="validation">Labels: {errors.labels}</text>
        )}
        {activeField && (
          <view className="property-popover">
            <view className="row between wrap">
              <text className="label">
                Edit{" "}
                {propertyFields.find(([field]) => field === activeField)?.[1]}
              </text>
              <Button
                onClick={() => {
                  setActiveField(undefined);
                  setOpenControl(undefined);
                  focusTrigger();
                }}
              >
                Close properties editor
              </Button>
            </view>
            {activeField === "priority" && (
              <TaskSelect
                control="priority"
                openControl={openControl}
                setOpenControl={setOpenControl}
                label="Priority"
                value={task.priority}
                options={priorityOptions}
                onChange={(priority) => edit({ priority })}
                disabled={disabled}
              />
            )}
            {activeField === "type" && (
              <TaskSelect
                control="type"
                openControl={openControl}
                setOpenControl={setOpenControl}
                label="Type"
                value={task.type}
                options={typeOptions}
                onChange={(type) => edit({ type })}
                disabled={disabled}
              />
            )}
            {activeField === "startDate" && (
              <Field
                label="Start date (YYYY-MM-DD)"
                value={task.startDate ?? ""}
                onChange={(value) =>
                  edit({ startDate: value.trim() || undefined })
                }
                disabled={disabled}
                error={errors.startDate}
                focus
                onKeyDown={(key) => {
                  if (key === "Escape") {
                    setActiveField(undefined);
                    focusTrigger();
                  }
                }}
              />
            )}
            {activeField === "dueDate" && (
              <Field
                label="Due date (YYYY-MM-DD)"
                value={task.dueDate ?? ""}
                onChange={(value) =>
                  edit({ dueDate: value.trim() || undefined })
                }
                disabled={disabled}
                error={errors.dueDate}
                focus
                onKeyDown={(key) => {
                  if (key === "Escape") {
                    setActiveField(undefined);
                    focusTrigger();
                  }
                }}
              />
            )}
            {activeField === "storyPoints" && (
              <Field
                label="Story points (whole number)"
                value={pointsText}
                onChange={setPointsText}
                disabled={disabled}
                error={errors.storyPoints}
                help="Leave empty for unassigned. 0 is a value."
                focus
                onKeyDown={(key) => {
                  if (key === "Escape") {
                    setActiveField(undefined);
                    focusTrigger();
                  }
                }}
              />
            )}
            {activeField === "labels" && (
              <>
                <TaskLabelSelect
                  openControl={openControl}
                  setOpenControl={setOpenControl}
                  selected={task.labelIds}
                  options={labelOptions}
                  disabled={ownedDisabled}
                  limit={MAX_LABELS}
                  onToggle={(id) => {
                    if (
                      !task.labelIds.includes(id) &&
                      task.labelIds.length >= MAX_LABELS
                    )
                      return;
                    edit({ labelIds: toggled(task.labelIds, id).sort() });
                  }}
                />
                <text className="muted">
                  {task.labelIds.length} of {MAX_LABELS} labels selected.
                </text>
                {errors.labels && (
                  <text className="validation">{errors.labels}</text>
                )}
              </>
            )}
            {activeField === "milestone" && (
              <TaskSelect
                control="milestone"
                openControl={openControl}
                setOpenControl={setOpenControl}
                label="Milestone"
                value={task.milestoneId ?? ""}
                options={single(task.milestoneId, milestones, "milestone")}
                onChange={(id) => edit({ milestoneId: id || undefined })}
                disabled={ownedDisabled}
              />
            )}
            {activeField === "cycle" && (
              <TaskSelect
                control="cycle"
                openControl={openControl}
                setOpenControl={setOpenControl}
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
            )}
          </view>
        )}
        {moved && (
          <Notice>
            Moving this task clears its labels, milestone, cycle and
            relationships; priority, type, dates and story points are kept. Save
            the move first, then assign labels, a milestone, a cycle and links
            in the new repository.
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
      </view>
    );
  }
  return (
    <view className="planning-section task-properties">
      <text className="heading" accessibility-heading>
        Properties
      </text>
      <TaskSelect
        control="priority"
        openControl={openControl}
        setOpenControl={setOpenControl}
        label="Priority"
        value={task.priority}
        options={priorityOptions}
        onChange={(priority) => edit({ priority })}
        disabled={disabled}
      />
      <TaskSelect
        control="type"
        openControl={openControl}
        setOpenControl={setOpenControl}
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
      <TaskLabelSelect
        openControl={openControl}
        setOpenControl={setOpenControl}
        selected={task.labelIds}
        options={labelOptions}
        disabled={ownedDisabled}
        limit={MAX_LABELS}
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
      <TaskSelect
        control="milestone"
        openControl={openControl}
        setOpenControl={setOpenControl}
        label="Milestone"
        value={task.milestoneId ?? ""}
        options={single(task.milestoneId, milestones, "milestone")}
        onChange={(id) => edit({ milestoneId: id || undefined })}
        disabled={ownedDisabled}
      />
      <TaskSelect
        control="cycle"
        openControl={openControl}
        setOpenControl={setOpenControl}
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
