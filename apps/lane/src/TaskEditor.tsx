// SPDX-License-Identifier: Apache-2.0
import { Captures } from "./Captures";
import type { UI } from "./serializable";
import { useEffect, useRef, useState } from "@lynx-js/react";
import { create } from "@bufbuild/protobuf";
import {
  LaneService,
  Proposal,
  TaskSchema,
  ResultKind,
  type EntityResult as ProtoEntityResult,
  type EntityVersion as ProtoEntityVersion,
} from "@repo/protos/lane/v1/lane_pb";
import { meta, newId, type LaneAdapter } from "./adapter";
import { Button, Choices, Dialog, Field, Markdown, Notice } from "./components";
import { TaskProperties } from "./TaskProperties";
import { TaskRelationships } from "./TaskRelationships";
import {
  deriveRelations,
  describeError,
  hasErrors,
  lookupOf,
  parsePoints,
  relationshipEdits,
  taskMetadata,
  taskRelationshipLines,
  mergeStoredRelated,
  validateTaskPlanning,
} from "./planning";
import { useTaskOptions, type PlanningState } from "./usePlanning";
import {
  createUITask,
  conflicted,
  editDraft,
  heads,
  historyChanges,
  progressOptions,
  promptForPresence,
  proposalOptions,
  repositoryName,
  taskOf,
  type Draft,
} from "./model";
import { hostAction } from "./runtime";
export function TaskEditor({
  adapter,
  draft,
  setDraft,
  current,
  repositories,
  planning,
  revision,
  openTask,
  onSaved,
  onClose,
}: {
  adapter: LaneAdapter;
  draft: Draft;
  setDraft: (draft: Draft) => void;
  current?: EntityResult;
  repositories: EntityResult[];
  planning: PlanningState;
  revision: number;
  // Opens a related task through the shared unsaved-change confirmation.
  openTask: (id: string) => void;
  onSaved: (r?: EntityResult) => void;
  onClose: () => void;
}) {
  const [tab, setTab] = useState("Description");
  const [preview, setPreview] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [repositoryUrl, setRepositoryUrl] = useState("");
  const [history, setHistory] = useState<EntityVersion[] | null>(null);
  const [historyCursor, setHistoryCursor] = useState("");
  const [prompts, setPrompts] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [resolve, setResolve] = useState(false);
  const [resolution, setResolution] = useState<EntityVersion>();
  // Preserve the request across an unchanged retry, including response loss.
  const [attempt, setAttempt] = useState<{ key: string; requestId: string }>();
  const readOnly = conflicted(current) || draft.deleted;
  // Story points stay text so a rejected value such as "1.5" is retained and explained.
  const pointsOf = (value?: number) =>
    value === undefined ? "" : String(value);
  const [pointsText, setPointsText] = useState(
    pointsOf(draft.task.storyPoints),
  );
  const [attempted, setAttempted] = useState(false);
  // Guards double submission before React re-renders the disabled state.
  const submitting = useRef(false);
  useEffect(() => {
    setPointsText(pointsOf(draft.task.storyPoints));
  }, [draft.id, draft.base]);
  const options = useTaskOptions(adapter, draft.task.repositoryId, revision);
  const moved =
    !!draft.base &&
    !!draft.baseTask &&
    draft.task.repositoryId !== draft.baseTask.repositoryId;
  useEffect(() => {
    if (
      draft.relations ||
      options.status !== "ready" ||
      !draft.task.repositoryId
    )
      return;
    // Removals made before the options loaded are edits against the saved state, so the
    // stored references of other tasks must not bring those links back.
    const own = draft.task.relatedTaskIds;
    const merged = mergeStoredRelated(
      draft.id,
      draft.baseTask && !moved ? draft.baseTask.relatedTaskIds : own,
      own,
      options.tasks,
    );
    const same = merged.related.join() === own.join();
    const baseSame =
      !draft.baseTask ||
      merged.base.join() === draft.baseTask.relatedTaskIds.join();
    // Related is symmetric: adopt the union of both stored sides as the saved state.
    setDraft({
      ...draft,
      task: same
        ? draft.task
        : createUITask({ ...draft.task, relatedTaskIds: merged.related }),
      baseTask:
        baseSame || !draft.baseTask
          ? draft.baseTask
          : { ...draft.baseTask, relatedTaskIds: merged.base },
      relations: deriveRelations(draft.id, options.tasks),
    });
  }, [draft.relations, options.status, options.tasks, draft.task.repositoryId]);
  const fieldErrors = attempted
    ? validateTaskPlanning(draft.task, pointsText)
    : {};
  const titles = new Map(options.tasks.map((t) => [t.id, t.title]));
  const lookup = lookupOf(
    planning.items,
    titles,
    new Map(
      repositories.map((r) => [r.version!.entityId, repositoryName(r.version)]),
    ),
  );
  const edit = (patch: Partial<Draft["task"]>) => {
    // Input controls can echo an unchanged value; that must not mark the draft dirty.
    const next = editDraft(draft, patch);
    if (next !== draft) setDraft(next);
  };
  // Repository-owned values belong to one repository: clear them on a change and
  // restore the saved values when the original repository is selected again.
  const changeRepository = (repositoryId: string) => {
    // Selecting the current repository again is not a move: keep every draft edit.
    if (repositoryId === draft.task.repositoryId) return;
    const base = draft.baseTask;
    const owned =
      base && repositoryId === base.repositoryId
        ? {
            labelIds: base.labelIds,
            milestoneId: base.milestoneId,
            cycleId: base.cycleId,
            parentTaskId: base.parentTaskId,
            dependsOnTaskIds: base.dependsOnTaskIds,
            relatedTaskIds: base.relatedTaskIds,
            duplicateOfTaskId: base.duplicateOfTaskId,
          }
        : {
            labelIds: [],
            milestoneId: undefined,
            cycleId: undefined,
            parentTaskId: undefined,
            dependsOnTaskIds: [],
            relatedTaskIds: [],
            duplicateOfTaskId: undefined,
          };
    setDraft({
      ...draft,
      task: createUITask({ ...draft.task, ...owned, repositoryId }),
      relations: undefined,
      dirty: true,
    });
  };
  const run = async (action: () => Promise<void>) => {
    if (submitting.current) return;
    submitting.current = true;
    setBusy(true);
    setError("");
    try {
      await action();
    } catch (e) {
      setError(describeError(e));
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  };
  const save = () =>
    run(async () => {
      if (!draft.task.title.trim()) throw new Error("Enter a title.");
      if (!draft.task.repositoryId && !repositoryUrl.trim())
        throw new Error("Choose a repository or enter its URL.");
      setAttempted(true);
      if (hasErrors(validateTaskPlanning(draft.task, pointsText)))
        throw new Error("Fix the fields marked above, then Save again.");
      const points = parsePoints(pointsText) as number | undefined;
      const task = createUITask({ ...draft.task, storyPoints: points });
      // A move clears links and the host removes incoming references itself.
      const edits = moved ? undefined : relationshipEdits(draft.relations);
      const key = JSON.stringify({
        base: draft.base,
        task,
        edits,
        repositoryUrl,
      });
      const requestId = attempt?.key === key ? attempt.requestId : newId();
      setAttempt({ key, requestId });
      const reply = draft.base
        ? await adapter.call(LaneService.method.saveTask, {
            meta: { requestId },
            id: draft.id,
            expectedVersionId: draft.base,
            task,
            relationshipEdits: edits,
          })
        : await adapter.call(LaneService.method.createTask, {
            meta: { requestId },
            id: draft.id,
            repository: {
              target: draft.task.repositoryId
                ? { case: "id", value: draft.task.repositoryId }
                : { case: "url", value: repositoryUrl },
            },
            title: draft.task.title,
            description: draft.task.description,
            prompt: draft.task.prompt,
            progress: draft.task.progress,
            proposal: draft.task.proposal,
            priority: task.priority,
            type: task.type,
            labelIds: task.labelIds,
            startDate: task.startDate,
            dueDate: task.dueDate,
            storyPoints: task.storyPoints,
            milestoneId: task.milestoneId,
            cycleId: task.cycleId,
            parentTaskId: task.parentTaskId,
            dependsOnTaskIds: task.dependsOnTaskIds,
            relatedTaskIds: task.relatedTaskIds,
            duplicateOfTaskId: task.duplicateOfTaskId,
            relationshipEdits: edits,
          });
      if (!reply.result)
        throw new Error(
          "The server returned no task. Your input is retained; refresh before retrying.",
        );
      if (reply.result.result === ResultKind.CONFLICT) {
        onSaved(reply.result);
        throw new Error(
          "Both versions were preserved. Compare and resolve the conflict.",
        );
      }
      // Refresh the same-repository options first so the new draft derives its
      // Subtasks and Blocks from the saved state, including affected tasks.
      await options.reload().catch(() => undefined);
      onSaved(reply.result);
    });
  const loadHistory = async (onlyPrompts: boolean, more = false) => {
    const r = await adapter.call(LaneService.method.listHistory, {
      id: draft.id,
      promptsOnly: onlyPrompts,
      page: { size: 100, cursor: more ? historyCursor : "" },
    });
    setPrompts(onlyPrompts);
    setHistory(more ? [...(history || []), ...r.versions] : r.versions);
    setHistoryCursor(r.nextCursor);
  };
  return (
    <view className="editor">
      <view className="row between">
        <text className="heading">
          {draft.base ? "Task details" : "New task"}
        </text>
        <Button onClick={onClose}>Back to list</Button>
      </view>
      <scroll-view className="editor-body" scroll-orientation="vertical">
        {draft.external && (
          <Notice>
            External changes are available. Your input is retained. Saving
            checks the original version.
          </Notice>
        )}
        {draft.deleted && (
          <Notice>
            This task was permanently deleted. Your unsaved input is retained
            for copying; it cannot be saved to this task.
          </Notice>
        )}
        {readOnly && !draft.deleted && (
          <Notice>
            This task has a conflict. Editing is paused until you compare and
            resolve all versions.
          </Notice>
        )}
        <Field
          label="Title"
          value={draft.task.title}
          onChange={(title) => edit({ title })}
          disabled={readOnly || busy}
          focus={!draft.base}
        />
        <view className="row wrap">
          <text>
            Proposal:{" "}
            {proposalOptions.find(([v]) => v === draft.task.proposal)?.[1]}
          </text>
          {draft.base && draft.task.proposal !== Proposal.REGULAR && (
            <>
              <Button
                disabled={busy || readOnly || draft.dirty}
                onClick={() =>
                  void run(async () => {
                    const r = await adapter.call(
                      LaneService.method.setProposal,
                      {
                        meta: meta(),
                        id: draft.id,
                        expectedVersionId: draft.base,
                        proposal: Proposal.REJECTED,
                      },
                    );
                    onSaved(r.result);
                  })
                }
              >
                Reject proposal
              </Button>
              <Button
                primary
                disabled={busy || readOnly || draft.dirty}
                onClick={() =>
                  void run(async () => {
                    const r = await adapter.call(
                      LaneService.method.setProposal,
                      {
                        meta: meta(),
                        id: draft.id,
                        expectedVersionId: draft.base,
                        proposal: Proposal.ACCEPTED,
                      },
                    );
                    onSaved(r.result);
                  })
                }
              >
                Accept proposal
              </Button>
            </>
          )}
        </view>
        {draft.dirty && (
          <text className="muted">
            Save your edits before a separate proposal or prompt restoration
            action.
          </text>
        )}
        <Choices
          label="Progress"
          value={draft.task.progress}
          options={progressOptions}
          onChange={(progress) => edit({ progress })}
          disabled={busy || readOnly}
        />
        <Choices
          label="Repository / move task"
          value={draft.task.repositoryId}
          options={[
            ...(!draft.base ? [["", "Use a repository URL"] as const] : []),
            ...repositories
              .filter((r) => r.version && !r.version.deleted)
              .map(
                (r) =>
                  [r.version!.entityId, repositoryName(r.version)] as const,
              ),
          ]}
          onChange={changeRepository}
          disabled={busy || readOnly}
        />
        {!draft.base && !draft.task.repositoryId && (
          <Field
            label="Repository URL"
            value={repositoryUrl}
            onChange={(value) => {
              setRepositoryUrl(value);
              setDraft({ ...draft, dirty: true });
            }}
          />
        )}
        <view className="row wrap">
          {["Description", "Prompt"].map((t) => (
            <Button key={t} primary={tab === t} onClick={() => setTab(t)}>
              {t}
            </Button>
          ))}
          <Button onClick={() => setPreview(!preview)}>
            {preview ? "Edit source" : "Preview Markdown"}
          </Button>
          <Button
            onClick={() =>
              void run(() =>
                hostAction(
                  "copyText",
                  tab === "Description"
                    ? draft.task.description
                    : draft.task.prompt || "",
                ),
              )
            }
          >
            Copy source
          </Button>
        </view>
        {tab === "Prompt" && (
          <Choices
            label="Prompt"
            value={draft.task.prompt !== undefined ? "present" : "absent"}
            options={[
              ["absent", "No prompt"],
              ["present", "Include prompt"],
            ]}
            onChange={(v) =>
              edit({ prompt: promptForPresence(draft.task.prompt, v) })
            }
            disabled={busy || readOnly}
          />
        )}
        {preview ? (
          <Markdown
            source={
              tab === "Description"
                ? draft.task.description
                : draft.task.prompt || ""
            }
          />
        ) : (
          <Field
            label={
              tab === "Description" ? "Description Markdown" : "Prompt Markdown"
            }
            multiline
            value={
              tab === "Description"
                ? draft.task.description
                : draft.task.prompt || ""
            }
            disabled={
              busy ||
              readOnly ||
              (tab === "Prompt" && draft.task.prompt === undefined)
            }
            onChange={(value) =>
              edit(
                tab === "Description"
                  ? { description: value }
                  : { prompt: value },
              )
            }
          />
        )}
        {tab === "Prompt" && (
          <Captures
            adapter={adapter}
            taskId={draft.id}
            captures={draft.task.captures}
          />
        )}
        <TaskProperties
          draft={draft}
          edit={edit}
          disabled={busy || readOnly}
          planning={planning}
          pointsText={pointsText}
          setPointsText={(value) => {
            if (value === pointsText) return;
            setPointsText(value);
            setDraft({ ...draft, dirty: true });
          }}
          errors={fieldErrors}
          moved={moved}
        />
        <TaskRelationships
          draft={draft}
          setDraft={setDraft}
          edit={edit}
          disabled={busy || readOnly}
          options={options}
          ownId={draft.id}
          moved={moved}
          openTask={openTask}
        />
        {error && <Notice>{error} Your input is retained.</Notice>}
      </scroll-view>
      <view className="toolbar row wrap">
        {draft.base && (
          <>
            <Button
              disabled={busy}
              onClick={() => void run(() => loadHistory(false))}
            >
              History
            </Button>
            <Button
              disabled={busy}
              onClick={() => void run(() => loadHistory(true))}
            >
              Prompt versions
            </Button>
            <Button disabled={busy} onClick={() => setConfirmDelete(true)}>
              Delete task
            </Button>
          </>
        )}
        {conflicted(current) && (
          <Button
            onClick={() => {
              setResolution(heads(current!)[0]);
              setResolve(true);
            }}
          >
            Compare conflict
          </Button>
        )}
        <Button onClick={onClose}>Cancel</Button>
        <Button primary disabled={busy || readOnly} onClick={() => void save()}>
          {busy ? "Saving…" : error ? "Retry Save" : "Save"}
        </Button>
      </view>
      {confirmDelete && (
        <Dialog
          title="Permanently delete task?"
          onClose={() => setConfirmDelete(false)}
        >
          <Notice>
            This removes the task, prompt and all history. There is no recovery.
            Other tasks keep existing and lose their links to this task.
          </Notice>
          <Button
            disabled={busy}
            onClick={() =>
              void run(async () => {
                await adapter.call(LaneService.method.deleteTask, {
                  meta: meta(),
                  id: draft.id,
                });
                setConfirmDelete(false);
                onSaved();
                onClose();
              })
            }
          >
            Delete permanently
          </Button>
        </Dialog>
      )}
      {history && (
        <Dialog
          title={prompts ? "Prompt versions" : "Task history"}
          onClose={() => setHistory(null)}
        >
          {history.map((version, index) => (
            <view className="card" key={version.versionId}>
              <text className="label">
                {version.createdAt
                  ? new Date(
                      Number(version.createdAt.seconds) * 1000,
                    ).toLocaleString()
                  : ""}
              </text>
              <text className="muted">
                {["Unknown", "App", "CLI", "MCP"][version.origin?.kind || 0]} ·{" "}
                {version.origin?.deviceName} ·{" "}
                {version.origin?.accountId || "Local account"}
              </text>
              {prompts ? (
                <>
                  <Markdown source={taskOf(version)?.prompt ?? "(No prompt)"} />
                  <Captures
                    adapter={adapter}
                    taskId={draft.id}
                    captures={taskOf(version)?.captures || []}
                  />
                  <Button
                    disabled={busy || readOnly || draft.dirty}
                    onClick={() =>
                      void run(async () => {
                        const r = await adapter.call(
                          LaneService.method.restorePrompt,
                          {
                            meta: meta(),
                            id: draft.id,
                            expectedVersionId: draft.base,
                            sourceVersionId: version.versionId,
                          },
                        );
                        setHistory(null);
                        onSaved(r.result);
                      })
                    }
                  >
                    Restore this prompt as a new version
                  </Button>
                </>
              ) : (
                historyChanges(history[index - 1], version, lookup).map((c) => (
                  <view key={c.field}>
                    <text className="label">{c.field}</text>
                    <text text-selection>Before: {c.before}</text>
                    <text text-selection>After: {c.after}</text>
                  </view>
                ))
              )}
            </view>
          ))}
          {historyCursor && (
            <Button onClick={() => void run(() => loadHistory(prompts, true))}>
              Load more history
            </Button>
          )}
        </Dialog>
      )}
      {resolve && current && (
        <Dialog
          title="Resolve whole-task conflict"
          onClose={() => setResolve(false)}
        >
          <Notice>
            Compare every field. Select a complete version, optionally edit it,
            then save a new resolution. A stale resolution requires refreshing
            first.
          </Notice>
          {heads(current).map((v) => (
            <view className="card" key={v.versionId}>
              <text>
                {v.versionId} · {v.origin?.deviceName}
              </text>
              <text text-selection>Title: {taskOf(v)?.title}</text>
              <text text-selection>Description: {taskOf(v)?.description}</text>
              <text text-selection>
                Prompt:{" "}
                {taskOf(v)?.prompt === undefined
                  ? "Not set"
                  : taskOf(v)?.prompt === ""
                    ? "Empty"
                    : taskOf(v)?.prompt}
              </text>
              <Captures
                adapter={adapter}
                taskId={draft.id}
                captures={taskOf(v)?.captures || []}
              />
              <text>
                Progress:{" "}
                {
                  progressOptions.find(
                    ([value]) => value === taskOf(v)?.progress,
                  )?.[1]
                }
              </text>
              <text>
                Proposal:{" "}
                {
                  proposalOptions.find(
                    ([value]) => value === taskOf(v)?.proposal,
                  )?.[1]
                }
              </text>
              <text>
                Repository:{" "}
                {repositoryName(
                  repositories.find(
                    (r) => r.version?.entityId === taskOf(v)?.repositoryId,
                  )?.version,
                )}
              </text>
              {taskOf(v) && (
                <PlanningSummary task={taskOf(v)!} lookup={lookup} />
              )}
              <Button
                primary={resolution?.versionId === v.versionId}
                onClick={() => setResolution(v)}
              >
                Use this version
              </Button>
            </view>
          ))}
          {resolution && taskOf(resolution) && (
            <>
              <Field
                label="Resolved title"
                value={taskOf(resolution)!.title}
                onChange={(title) =>
                  setResolution({
                    ...resolution,
                    body: {
                      case: "task",
                      value: createUITask({
                        ...taskOf(resolution)!,
                        title,
                      }),
                    },
                  })
                }
              />
              <Field
                label="Resolved description"
                multiline
                value={taskOf(resolution)!.description}
                onChange={(description) =>
                  setResolution({
                    ...resolution,
                    body: {
                      case: "task",
                      value: createUITask({
                        ...taskOf(resolution)!,
                        description,
                      }),
                    },
                  })
                }
              />
              <Choices
                label="Resolved prompt presence"
                value={
                  taskOf(resolution)!.prompt === undefined
                    ? "absent"
                    : "present"
                }
                options={[
                  ["absent", "No prompt"],
                  ["present", "Include prompt"],
                ]}
                onChange={(value) =>
                  setResolution({
                    ...resolution,
                    body: {
                      case: "task",
                      value: createUITask({
                        ...taskOf(resolution)!,
                        prompt:
                          value === "absent"
                            ? undefined
                            : taskOf(resolution)!.prompt || "",
                      }),
                    },
                  })
                }
              />
              <Field
                label="Resolved prompt"
                multiline
                value={taskOf(resolution)!.prompt || ""}
                onChange={(prompt) =>
                  setResolution({
                    ...resolution,
                    body: {
                      case: "task",
                      value: createUITask({
                        ...taskOf(resolution)!,
                        prompt,
                      }),
                    },
                  })
                }
              />
              <Choices
                label="Resolved progress"
                value={taskOf(resolution)!.progress}
                options={progressOptions}
                onChange={(progress) =>
                  setResolution({
                    ...resolution,
                    body: {
                      case: "task",
                      value: createUITask({
                        ...taskOf(resolution)!,
                        progress,
                      }),
                    },
                  })
                }
              />
              <Choices
                label="Resolved proposal"
                value={taskOf(resolution)!.proposal}
                options={proposalOptions}
                onChange={(proposal) =>
                  setResolution({
                    ...resolution,
                    body: {
                      case: "task",
                      value: createUITask({
                        ...taskOf(resolution)!,
                        proposal,
                      }),
                    },
                  })
                }
              />
              <Choices
                label="Resolved repository"
                value={taskOf(resolution)!.repositoryId}
                options={repositories.map(
                  (r) =>
                    [r.version!.entityId, repositoryName(r.version)] as const,
                )}
                onChange={(repositoryId) =>
                  setResolution({
                    ...resolution,
                    body: {
                      case: "task",
                      value: createUITask({
                        ...taskOf(resolution)!,
                        repositoryId,
                      }),
                    },
                  })
                }
              />
            </>
          )}
          <Button
            primary
            disabled={busy || !resolution}
            onClick={() =>
              void run(async () => {
                const r = await adapter.call(
                  LaneService.method.resolveConflict,
                  {
                    meta: meta(),
                    id: draft.id,
                    expectedHeadIds: heads(current)
                      .map((v) => v.versionId)
                      .sort(),
                    resolved: resolution,
                  },
                );
                if (!r.result || r.result.result === ResultKind.CONFLICT)
                  throw new Error(
                    "The conflict changed. Your resolution is retained. Close this dialog and refresh before resolving again.",
                  );
                setResolve(false);
                onSaved(r.result);
              })
            }
          >
            Save resolution
          </Button>
          {error && <Notice>{error}</Notice>}
        </Dialog>
      )}
    </view>
  );
}

function PlanningSummary({
  task,
  lookup,
}: {
  task: NonNullable<ReturnType<typeof taskOf>>;
  lookup: ReturnType<typeof lookupOf>;
}) {
  const meta = taskMetadata(task, lookup, false);
  // Comparisons list each relationship target, not the compact list-row counts.
  const relationships = taskRelationshipLines(task, lookup);
  return (
    <>
      <text text-selection>
        Planning: {meta.summary.join(" · ") || "No planning values"}
      </text>
      <text text-selection>Labels: {meta.labels.join(", ") || "None"}</text>
      {relationships.length ? (
        relationships.map((line) => (
          <text text-selection key={line}>
            {line}
          </text>
        ))
      ) : (
        <text text-selection>Links: None</text>
      )}
    </>
  );
}

type EntityResult = UI<ProtoEntityResult>;

type EntityVersion = UI<ProtoEntityVersion>;
