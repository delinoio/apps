// SPDX-License-Identifier: Apache-2.0
import { useState } from "@lynx-js/react";
import type { UI } from "./serializable";
import type { Task as ProtoTask } from "@repo/protos/lane/v1/lane_pb";
import { Button, Field, Notice } from "./components";
import type { Draft } from "./model";
import type { TaskRef } from "./planning";
import type { TaskOptions } from "./usePlanning";

type Task = UI<ProtoTask>;
type Kind =
  "parent" | "subtasks" | "dependsOn" | "blocks" | "related" | "duplicate";
const sections: { kind: Kind; title: string; single: boolean; noun: string }[] =
  [
    { kind: "parent", title: "Parent", single: true, noun: "parent" },
    { kind: "subtasks", title: "Subtasks", single: false, noun: "subtask" },
    {
      kind: "dependsOn",
      title: "Depends on",
      single: false,
      noun: "dependency",
    },
    { kind: "blocks", title: "Blocks", single: false, noun: "blocked task" },
    { kind: "related", title: "Related", single: false, noun: "related task" },
    {
      kind: "duplicate",
      title: "Duplicate of",
      single: true,
      noun: "original task",
    },
  ];
const MAX_RESULTS = 8;

function TaskPicker({
  title,
  options,
  exclude,
  disabled,
  onPick,
  onClose,
}: {
  title: string;
  options: TaskRef[];
  exclude: string[];
  // While the editor is saving or read-only, the picker must not change the draft.
  disabled: boolean;
  onPick: (id: string) => void;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  // Titles need not be unique, so refining the search cannot always separate
  // matches: let the user reveal further results page by page.
  const [shown, setShown] = useState(MAX_RESULTS);
  const needle = query.trim().toLowerCase();
  const matches = options
    .filter(
      (o) =>
        !exclude.includes(o.id) &&
        (!needle || o.title.toLowerCase().includes(needle)),
    )
    .sort((a, b) => a.title.localeCompare(b.title) || a.id.localeCompare(b.id));
  return (
    <view className="picker">
      <Field
        label={`Search tasks for ${title}`}
        value={query}
        onChange={(value) => {
          setQuery(value);
          setShown(MAX_RESULTS);
        }}
        disabled={disabled}
        focus
      />
      {matches.length === 0 ? (
        <text className="muted">
          {options.length === 0
            ? "This repository has no other tasks to link."
            : "No matching tasks in this repository."}
        </text>
      ) : (
        matches.slice(0, shown).map((m) => (
          <Button
            key={m.id}
            label={`Add ${m.title} to ${title}`}
            disabled={disabled}
            onClick={() => onPick(m.id)}
          >
            {m.title}
          </Button>
        ))
      )}
      {matches.length > shown && (
        <>
          <text className="muted">
            {matches.length - shown} more tasks match.
          </text>
          <Button onClick={() => setShown(shown + MAX_RESULTS)}>
            Show more matching tasks
          </Button>
        </>
      )}
      <Button onClick={onClose}>Cancel adding</Button>
    </view>
  );
}

export function TaskRelationships({
  draft,
  setDraft,
  edit,
  disabled,
  options,
  ownId,
  moved,
  openTask,
}: {
  draft: Draft;
  setDraft: (draft: Draft) => void;
  edit: (patch: Partial<Task>) => void;
  disabled: boolean;
  options: TaskOptions;
  ownId: string;
  moved: boolean;
  openTask: (id: string) => void;
}) {
  const [adding, setAdding] = useState<Kind>();
  const task = draft.task;
  const unavailable = !task.repositoryId || moved;
  const candidates = options.tasks.filter((t) => t.id !== ownId);
  const titleOf = (id: string) =>
    options.tasks.find((t) => t.id === id)?.title ?? "(unavailable task)";
  const get = (kind: Kind): string[] => {
    switch (kind) {
      case "parent":
        return task.parentTaskId ? [task.parentTaskId] : [];
      case "duplicate":
        return task.duplicateOfTaskId ? [task.duplicateOfTaskId] : [];
      case "dependsOn":
        return task.dependsOnTaskIds;
      case "related":
        return task.relatedTaskIds;
      case "subtasks":
        return draft.relations?.subtasks ?? [];
      case "blocks":
        return draft.relations?.blocks ?? [];
    }
  };
  const set = (kind: Kind, ids: string[]) => {
    const next = [...new Set(ids)].sort();
    switch (kind) {
      case "parent":
        return edit({ parentTaskId: ids[0] });
      case "duplicate":
        return edit({ duplicateOfTaskId: ids[0] });
      case "dependsOn":
        return edit({ dependsOnTaskIds: next });
      case "related":
        return edit({ relatedTaskIds: next });
      case "subtasks":
      case "blocks":
        if (!draft.relations) return;
        return setDraft({
          ...draft,
          relations: { ...draft.relations, [kind]: next },
          dirty: true,
        });
    }
  };
  const derived = (kind: Kind) => kind === "subtasks" || kind === "blocks";
  return (
    <view className="planning-section">
      <text className="heading" accessibility-heading>
        Relationships
      </text>
      <text className="muted">
        Links describe work only: they never change progress, proposals or
        completion. Linked tasks must be in the same repository. Changes stay in
        this draft until you Save.
      </text>
      {moved && (
        <Notice>
          Relationships are cleared when a task moves. Save the move first, then
          link tasks in the new repository.
        </Notice>
      )}
      {!task.repositoryId && !moved && (
        <Notice>Choose an existing repository to link tasks.</Notice>
      )}
      {!unavailable && options.status === "loading" && (
        <text className="muted">Loading tasks of this repository…</text>
      )}
      {!unavailable && options.status === "error" && (
        <Notice>
          Could not load this repository’s tasks. {options.error}{" "}
          <Button onClick={options.retry}>Retry loading tasks</Button>
        </Notice>
      )}
      {sections.map(({ kind, title, single, noun }) => {
        const ids = get(kind);
        const pending =
          derived(kind) && !draft.relations && options.status !== "error";
        const picking = adding === kind;
        const canAdd = !(single && ids.length > 0);
        return (
          <view className="relation" key={kind}>
            <text className="label" accessibility-heading>
              {title}
            </text>
            {derived(kind) && (
              <text className="muted">
                {kind === "subtasks"
                  ? "Tasks whose parent is this task."
                  : "Tasks that depend on this task."}
              </text>
            )}
            {ids.length === 0 && !pending && (
              <text className="muted">None</text>
            )}
            {pending && !unavailable && <text className="muted">Loading…</text>}
            {ids.map((id) => (
              <view className="linked row wrap" key={id}>
                <text className="linked-title">{titleOf(id)}</text>
                <Button
                  label={`Open ${titleOf(id)}`}
                  onClick={() => openTask(id)}
                >
                  Open
                </Button>
                {single && (
                  <Button
                    disabled={disabled || unavailable}
                    onClick={() => setAdding(kind)}
                  >
                    {kind === "parent" ? "Change parent" : "Change original"}
                  </Button>
                )}
                <Button
                  disabled={disabled || unavailable}
                  label={`Remove ${titleOf(id)} from ${title}`}
                  onClick={() =>
                    set(
                      kind,
                      ids.filter((x) => x !== id),
                    )
                  }
                >
                  Remove
                </Button>
              </view>
            ))}
            {picking ? (
              <TaskPicker
                key={kind}
                title={title}
                options={candidates}
                exclude={single ? [] : ids}
                disabled={disabled || unavailable}
                onPick={(id) => {
                  // A save in flight already built its request from the current draft.
                  if (disabled || unavailable) return;
                  set(kind, single ? [id] : [...ids, id]);
                  setAdding(undefined);
                }}
                onClose={() => setAdding(undefined)}
              />
            ) : (
              canAdd && (
                <Button
                  disabled={
                    disabled ||
                    unavailable ||
                    options.status !== "ready" ||
                    (derived(kind) && !draft.relations)
                  }
                  label={`Add ${noun}`}
                  onClick={() => setAdding(kind)}
                >
                  {`Add ${noun}`}
                </Button>
              )
            )}
          </view>
        );
      })}
    </view>
  );
}
