// SPDX-License-Identifier: Apache-2.0
import { Captures } from "./Captures";
import type { UI } from "./serializable";
import { useEffect, useRef, useState } from "@lynx-js/react";
import { create } from "@bufbuild/protobuf";
import {
  LaneService,
  OriginKind,
  Proposal,
  TaskSchema,
  ResultKind,
  type EntityResult as ProtoEntityResult,
  type EntityVersion as ProtoEntityVersion,
} from "@repo/protos/lane/v1/lane_pb";
import { LaneError, meta, newId, type LaneAdapter } from "./adapter";
import { RequestAttempt, SpecializedAction } from "./requestAttempt";
import { Button, Choices, Dialog, Field, Markdown, Notice } from "./components";
import { TaskProperties } from "./TaskProperties";
import { TaskRelationships } from "./TaskRelationships";
import { TaskSelect } from "./TaskControls";
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
  headIds,
  heads,
  historyEntryChanges,
  historyBaseline,
  progressOptions,
  promptForPresence,
  proposalOptions,
  repositoryName,
  resolutionStale,
  taskOf,
  type Draft,
} from "./model";
import { hostAction } from "./runtime";
import type { EditorCompletion } from "./editorLifetime";

enum HistoryMode {
  Task = "task",
  Prompts = "prompts",
}

type HistorySession = {
  mode: HistoryMode;
  versions: EntityVersion[];
  cursor: string;
  loading: boolean;
  loadingMore: boolean;
  error: string;
  retryable: boolean;
  restoreError: string;
  restoreErrorId: string;
  restoreRetryable: boolean;
  restoringId: string;
};

const historyFieldLabels: Record<string, string> = {
  title: "Title",
  description: "Description",
  progress: "Progress",
  proposal: "Proposal",
  repositoryId: "Repository",
};

function historyOrigin(version: EntityVersion): string {
  const kind = version.origin?.kind;
  const name =
    kind === OriginKind.APP
      ? "App"
      : kind === OriginKind.CLI
        ? "CLI"
        : kind === OriginKind.MCP
          ? "MCP"
          : "Unknown";
  return [
    name,
    version.origin?.deviceName,
    version.origin?.accountId || "Local account",
  ]
    .filter(Boolean)
    .join(" · ");
}

function historyRetryable(error: unknown): boolean {
  if (!(error instanceof LaneError)) return true;
  if (
    /PERMISSION|UNAUTH|FORBIDDEN|CONFLICT|STALE|DELETED|NOT_FOUND/.test(
      error.detail.reason,
    )
  )
    return false;
  return error.detail.retryable !== false;
}
export function TaskEditor({
  adapter,
  draft,
  setDraft,
  current,
  repositories,
  planning,
  revision,
  openTask,
  beginCompletion,
  onClose,
  onBack,
  busyChanged,
}: {
  adapter: LaneAdapter;
  draft: Draft;
  setDraft: (draft: Draft) => void;
  current?: EntityResult;
  repositories: EntityResult[];
  planning: PlanningState;
  revision: number;
  // Opens a related task in its own workspace tab.
  openTask: (id: string) => void;
  // Binds a mutation to the editor lifetime that submits it. Every mutation result
  // goes through the returned completion so a late result from an ended editor
  // cannot replace, clean, reopen or close another editor.
  beginCompletion: () => EditorCompletion<EntityResult>;
  onClose: () => void;
  onBack?: () => void;
  busyChanged?: (busy: boolean) => void;
}) {
  const [tab, setTab] = useState("Description");
  const [preview, setPreview] = useState(false);
  const [openControl, setOpenControl] = useState<string>();
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    busyChanged?.(busy);
  }, [busy, busyChanged]);
  const [error, setError] = useState("");
  const [repositoryUrl, setRepositoryUrl] = useState("");
  const [historySession, setHistorySession] = useState<HistorySession>();
  const historyOwner = useRef({ generation: 0, pending: false, active: false });
  // Retained task tabs can mount several editors; each dialog must return focus
  // to its own launcher rather than a launcher in another tab.
  const [historyTriggers] = useState(() => ({
    task: `task-history-${Math.random().toString(36).slice(2)}`,
    prompts: `prompt-versions-${Math.random().toString(36).slice(2)}`,
  }));
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [resolve, setResolve] = useState(false);
  const [resolution, setResolution] = useState<EntityVersion>();
  const [selectedHeadId, setSelectedHeadId] = useState<string>();
  const [compareFocusId] = useState(
    () => "compare-conflict-" + Math.random().toString(36).slice(2),
  );
  // Head set the user reviewed when the resolution session began. The resolution is
  // always submitted against this snapshot, never against a later refreshed `current`.
  const [reviewedHeads, setReviewedHeads] = useState<string[]>();
  // An edited resolution from an earlier session whose heads changed; kept for
  // inspection and explicit reuse after the current versions are reviewed again.
  const [retained, setRetained] = useState<EntityVersion>();
  const resolutionStaleNow = resolutionStale(reviewedHeads, current);
  const currentHeads = current ? heads(current) : [];
  // Preserve the request across an unchanged retry, including response loss.
  const [attempt, setAttempt] = useState<{ key: string; requestId: string }>();
  // Proposal decisions and prompt restorations keep their request identity across
  // a rejected reply and a clean-draft refresh, so an unchanged retry replays the
  // committed action. The instance lives as long as this editor stays mounted.
  const [actions] = useState(() => new RequestAttempt(newId));
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
  const editResolution = (
    patch: Partial<NonNullable<ReturnType<typeof taskOf>>>,
  ) => {
    if (submitting.current || !resolution || !taskOf(resolution)) return;
    setResolution({
      ...resolution,
      body: {
        case: "task",
        value: createUITask({ ...taskOf(resolution)!, ...patch }),
      },
    });
  };
  useEffect(() => {
    setHistorySession(undefined);
    return () => {
      historyOwner.current.generation++;
      historyOwner.current.pending = false;
      historyOwner.current.active = false;
    };
  }, [adapter, draft.id]);
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
  const run = async (
    action: (completion: EditorCompletion<EntityResult>) => Promise<void>,
  ) => {
    if (submitting.current) return;
    submitting.current = true;
    setBusy(true);
    setError("");
    try {
      await action(beginCompletion());
    } catch (e) {
      setError(describeError(e));
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  };
  const save = () =>
    run(async (completion) => {
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
        completion.saved(reply.result);
        throw new Error(
          "Both versions were preserved. Compare and resolve the conflict.",
        );
      }
      // Refresh the same-repository options first so the new draft derives its
      // Subtasks and Blocks from the saved state, including affected tasks.
      await options.reload().catch(() => undefined);
      completion.saved(reply.result);
    });
  const closeHistory = () => {
    historyOwner.current.generation++;
    historyOwner.current.pending = false;
    historyOwner.current.active = false;
    setHistorySession(undefined);
  };
  const loadHistory = async (mode: HistoryMode, cursor = "") => {
    if (historyOwner.current.pending) return;
    historyOwner.current.pending = true;
    const generation = historyOwner.current.generation;
    const more = !!cursor;
    setHistorySession((session) =>
      session && session.mode === mode
        ? {
            ...session,
            loading: !more,
            loadingMore: more,
            error: "",
          }
        : session,
    );
    try {
      const reply = await adapter.call(LaneService.method.listHistory, {
        id: draft.id,
        promptsOnly: mode === HistoryMode.Prompts,
        page: { size: 100, cursor },
      });
      if (historyOwner.current.generation !== generation) return;
      setHistorySession((session) =>
        session && session.mode === mode
          ? {
              ...session,
              versions: more
                ? [...session.versions, ...reply.versions]
                : reply.versions,
              cursor: reply.nextCursor,
              loading: false,
              loadingMore: false,
              error: "",
            }
          : session,
      );
    } catch (cause) {
      if (historyOwner.current.generation !== generation) return;
      setHistorySession((session) =>
        session && session.mode === mode
          ? {
              ...session,
              loading: false,
              loadingMore: false,
              error: describeError(cause),
              retryable: historyRetryable(cause),
            }
          : session,
      );
    } finally {
      if (historyOwner.current.generation === generation)
        historyOwner.current.pending = false;
    }
  };
  const openHistory = (mode: HistoryMode) => {
    if (submitting.current || historyOwner.current.active) return;
    historyOwner.current.generation++;
    historyOwner.current.active = true;
    setHistorySession({
      mode,
      versions: [],
      cursor: "",
      loading: true,
      loadingMore: false,
      error: "",
      retryable: false,
      restoreError: "",
      restoreErrorId: "",
      restoreRetryable: false,
      restoringId: "",
    });
    void loadHistory(mode);
  };
  const restoreHistoryPrompt = async (version: EntityVersion) => {
    if (
      submitting.current ||
      !historySession ||
      historySession.mode !== HistoryMode.Prompts ||
      readOnly ||
      draft.dirty ||
      busy
    )
      return;
    submitting.current = true;
    const generation = historyOwner.current.generation;
    setBusy(true);
    setHistorySession((session) =>
      session
        ? {
            ...session,
            restoringId: version.versionId,
            restoreError: "",
            restoreErrorId: "",
            restoreRetryable: false,
          }
        : session,
    );
    const completion = beginCompletion();
    try {
      const reply = await actions.submit(
        {
          action: SpecializedAction.RestorePrompt,
          id: draft.id,
          expectedVersionId: draft.base,
          sourceVersionId: version.versionId,
        },
        (requestId) =>
          adapter.call(LaneService.method.restorePrompt, {
            meta: { requestId },
            id: draft.id,
            expectedVersionId: draft.base,
            sourceVersionId: version.versionId,
          }),
      );
      if (!reply.result)
        throw new Error("The server returned no task. Your input is retained.");
      completion.saved(reply.result);
      if (reply.result.result === ResultKind.CONFLICT)
        throw new Error(
          "The task changed. Resolve the conflict before restoring a prompt.",
        );
      if (historyOwner.current.generation === generation) closeHistory();
    } catch (cause) {
      if (historyOwner.current.generation === generation)
        setHistorySession((session) =>
          session
            ? {
                ...session,
                restoreError: describeError(cause),
                restoreErrorId: version.versionId,
                restoreRetryable: historyRetryable(cause),
              }
            : session,
        );
    } finally {
      submitting.current = false;
      setBusy(false);
      if (historyOwner.current.generation === generation)
        setHistorySession((session) =>
          session ? { ...session, restoringId: "" } : session,
        );
    }
  };
  return (
    <view className="editor">
      <view className="row between">
        <text className="heading">
          {draft.base ? "Task details" : "New task"}
        </text>
        <Button onClick={onBack ?? onClose}>Back to list</Button>
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
                  void run(async (completion) => {
                    const r = await actions.submit(
                      {
                        action: SpecializedAction.SetProposal,
                        id: draft.id,
                        expectedVersionId: draft.base,
                        proposal: Proposal.REJECTED,
                      },
                      (requestId) =>
                        adapter.call(LaneService.method.setProposal, {
                          meta: { requestId },
                          id: draft.id,
                          expectedVersionId: draft.base,
                          proposal: Proposal.REJECTED,
                        }),
                    );
                    completion.saved(r.result);
                  })
                }
              >
                Reject proposal
              </Button>
              <Button
                primary
                disabled={busy || readOnly || draft.dirty}
                onClick={() =>
                  void run(async (completion) => {
                    const r = await actions.submit(
                      {
                        action: SpecializedAction.SetProposal,
                        id: draft.id,
                        expectedVersionId: draft.base,
                        proposal: Proposal.ACCEPTED,
                      },
                      (requestId) =>
                        adapter.call(LaneService.method.setProposal, {
                          meta: { requestId },
                          id: draft.id,
                          expectedVersionId: draft.base,
                          proposal: Proposal.ACCEPTED,
                        }),
                    );
                    completion.saved(r.result);
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
        <TaskSelect
          control="progress"
          openControl={openControl}
          setOpenControl={setOpenControl}
          label="Progress"
          value={draft.task.progress}
          options={progressOptions}
          onChange={(progress) => edit({ progress })}
          disabled={busy || readOnly}
        />
        <TaskSelect
          control="repository"
          openControl={openControl}
          setOpenControl={setOpenControl}
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
            disabled={busy || readOnly}
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
          openControl={openControl}
          setOpenControl={setOpenControl}
        />
        <TaskRelationships
          draft={draft}
          setDraft={setDraft}
          edit={edit}
          disabled={busy || readOnly}
          options={options}
          ownId={draft.id}
          moved={moved}
          openControl={openControl}
          setOpenControl={setOpenControl}
          openTask={openTask}
        />
        {error && <Notice>{error} Your input is retained.</Notice>}
      </scroll-view>
      <view className="toolbar row wrap">
        {draft.base && (
          <>
            <Button
              id={historyTriggers.task}
              disabled={busy}
              onClick={() => openHistory(HistoryMode.Task)}
            >
              History
            </Button>
            <Button
              id={historyTriggers.prompts}
              disabled={busy}
              onClick={() => openHistory(HistoryMode.Prompts)}
            >
              Prompt versions
            </Button>
            <Button
              disabled={busy}
              onClick={() => {
                // An earlier save or action failure is not a deletion failure.
                setError("");
                setConfirmDelete(true);
              }}
            >
              Delete task
            </Button>
          </>
        )}
        {conflicted(current) && (
          <Button
            id={compareFocusId}
            onClick={() => {
              // Reopening keeps an existing session; a stale one stays blocked until
              // the user explicitly reviews the current versions.
              if (!reviewedHeads) {
                setReviewedHeads(headIds(current));
                setResolution(currentHeads[0]);
                setSelectedHeadId(currentHeads[0]?.versionId);
              }
              setResolve(true);
            }}
          >
            Compare conflict
          </Button>
        )}
        <Button disabled={busy} onClick={onClose}>
          Cancel
        </Button>
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
          {/* A rejected deletion leaves this dialog open; show the failure here, where the
              user is acting, instead of only in the editor underneath. */}
          {error && <Notice>{error}</Notice>}
          <Button
            disabled={busy}
            onClick={() =>
              void run(async (completion) => {
                await adapter.call(LaneService.method.deleteTask, {
                  meta: meta(),
                  id: draft.id,
                });
                setConfirmDelete(false);
                completion.saved();
                completion.close();
              })
            }
          >
            Delete permanently
          </Button>
        </Dialog>
      )}
      {historySession && (
        <Dialog
          title={
            historySession.mode === HistoryMode.Prompts
              ? "Prompt versions"
              : "Task history"
          }
          className="history-dialog"
          returnFocusId={
            historySession.mode === HistoryMode.Prompts
              ? historyTriggers.prompts
              : historyTriggers.task
          }
          onClose={closeHistory}
          footer={
            <view className="history-footer">
              {historySession.error && (
                <Notice>{historySession.error} Your input is retained.</Notice>
              )}
              {historySession.error &&
                historySession.retryable &&
                !historySession.loading && (
                  <Button
                    disabled={historySession.loadingMore}
                    onClick={() =>
                      void loadHistory(
                        historySession.mode,
                        historySession.cursor,
                      )
                    }
                  >
                    Retry loading history
                  </Button>
                )}
              {historySession.loadingMore && (
                <text accessibility-live-region="polite">
                  Loading more history…
                </text>
              )}
              {historySession.cursor && !historySession.error && (
                <Button
                  disabled={
                    historySession.loadingMore || historySession.loading
                  }
                  onClick={() =>
                    void loadHistory(historySession.mode, historySession.cursor)
                  }
                >
                  Load more history
                </Button>
              )}
            </view>
          }
        >
          {historySession.mode === HistoryMode.Task && (
            <text className="history-intro">
              Changes to this task, with time and origin.
            </text>
          )}
          {historySession.loading && (
            <text accessibility-live-region="polite">Loading history…</text>
          )}
          {!historySession.loading &&
            !historySession.error &&
            historySession.versions.length === 0 && (
              <text className="empty">
                {historySession.mode === HistoryMode.Prompts
                  ? "No prompt versions available."
                  : "No task history available."}
              </text>
            )}
          {historySession.versions.map((version) => (
            <view className="card history-card" key={version.versionId}>
              {version.createdAt && (
                <text className="history-time">
                  {new Date(
                    Number(version.createdAt.seconds) * 1000,
                  ).toLocaleString()}
                </text>
              )}
              <text className="history-origin">{historyOrigin(version)}</text>
              {historySession.mode === HistoryMode.Prompts ? (
                <>
                  {taskOf(version)?.prompt === undefined ? (
                    <text className="muted">(No prompt)</text>
                  ) : taskOf(version)?.prompt === "" ? (
                    <text className="muted">Empty prompt</text>
                  ) : (
                    <Markdown source={taskOf(version)!.prompt!} />
                  )}
                  <Captures
                    adapter={adapter}
                    taskId={draft.id}
                    captures={taskOf(version)?.captures || []}
                  />
                  <Button
                    disabled={
                      busy ||
                      readOnly ||
                      draft.dirty ||
                      (historySession.restoreErrorId === version.versionId &&
                        !historySession.restoreRetryable)
                    }
                    onClick={() => void restoreHistoryPrompt(version)}
                  >
                    {historySession.restoringId === version.versionId
                      ? "Restoring…"
                      : historySession.restoreErrorId === version.versionId
                        ? "Retry restoring this prompt"
                        : "Restore this prompt as a new version"}
                  </Button>
                  {historySession.restoreErrorId === version.versionId && (
                    <Notice>
                      {historySession.restoreError} Your input is retained.
                    </Notice>
                  )}
                </>
              ) : (
                <>
                  {historyBaseline(historySession.versions, version).kind ===
                    "pending" && (
                    <text className="muted">
                      Earlier version is not loaded yet.
                    </text>
                  )}
                  {historyBaseline(historySession.versions, version).kind ===
                    "merge" && (
                    <text className="muted">
                      This resolution has multiple parent versions.
                    </text>
                  )}
                  {historyEntryChanges(
                    historySession.versions,
                    version,
                    lookup,
                  ).map((change) => (
                    <view className="history-change" key={change.field}>
                      <text className="history-field">
                        {historyFieldLabels[change.field] ?? change.field}
                      </text>
                      <view className="history-values">
                        <view className="history-value">
                          <text className="history-value-label">Before</text>
                          <text text-selection>{change.before}</text>
                        </view>
                        <view className="history-value">
                          <text className="history-value-label">After</text>
                          <text text-selection>{change.after}</text>
                        </view>
                      </view>
                    </view>
                  ))}
                </>
              )}
            </view>
          ))}
          {historySession.mode === HistoryMode.Prompts && (
            <>
              {draft.dirty && (
                <Notice>Save your changes before restoring a prompt.</Notice>
              )}
              {conflicted(current) && (
                <Notice>
                  Resolve the task conflict before restoring a prompt.
                </Notice>
              )}
              {draft.deleted && (
                <Notice>
                  This task was deleted. Its prompt cannot be restored.
                </Notice>
              )}
            </>
          )}
        </Dialog>
      )}
      {resolve && current && (
        <Dialog
          title="Resolve whole-task conflict"
          onClose={() => setResolve(false)}
          focusHeading
          returnFocusId={compareFocusId}
          className="conflict-dialog"
        >
          <text className="conflict-instructions">
            Compare the current versions, choose a complete version, then edit
            Your resolution. Saving creates one new version.
          </text>
          <view
            className="conflict-review-notice"
            accessibility-element
            accessibility-traits="text"
          >
            <text>
              Save checks exactly the versions you reviewed. Changes to the head
              set require another review.
            </text>
          </view>
          {resolutionStaleNow && (
            <Notice>
              {headIds(current).length < 2
                ? "The conflict was resolved elsewhere, so this resolution can no longer be saved. Your edits are kept below for reference."
                : "The task changed after you started this resolution, so it can no longer be saved. Your edits are kept below. Review the current versions to start a new resolution."}
            </Notice>
          )}
          {resolutionStaleNow && headIds(current).length > 1 && (
            <Button
              primary
              disabled={busy}
              onClick={() => {
                if (resolution) setRetained(resolution);
                setResolution(undefined);
                setSelectedHeadId(undefined);
                setReviewedHeads(headIds(current));
              }}
            >
              Review current versions
            </Button>
          )}
          <view className="conflict-section">
            <text className="heading" accessibility-heading>
              Current versions
            </text>
            <view className="conflict-heads">
              {currentHeads.map((v, index) => (
                <view
                  className={`conflict-head ${selectedHeadId === v.versionId ? "selected" : ""}`}
                  data-selected={
                    selectedHeadId === v.versionId ? "true" : "false"
                  }
                  key={v.versionId}
                >
                  <view className="row between wrap">
                    <text
                      className="conflict-version-title"
                      accessibility-heading
                    >
                      Version {index + 1}
                    </text>
                    {selectedHeadId === v.versionId && (
                      <text
                        className="conflict-selected"
                        accessibility-element
                        accessibility-traits="text"
                      >
                        ✓ Selected
                      </text>
                    )}
                  </view>
                  <text className="conflict-value" text-selection>
                    Version ID: {v.versionId}
                  </text>
                  <text className="conflict-value" text-selection>
                    Origin: {v.origin?.deviceName || "Not set"}
                  </text>
                  <text className="conflict-value" text-selection>
                    Title: {taskOf(v)?.title}
                  </text>
                  <text className="conflict-value" text-selection>
                    Description: {taskOf(v)?.description || "No description"}
                  </text>
                  <text className="conflict-value" text-selection>
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
                  <text className="conflict-value" text-selection>
                    Progress:{" "}
                    {
                      progressOptions.find(
                        ([value]) => value === taskOf(v)?.progress,
                      )?.[1]
                    }
                  </text>
                  <text className="conflict-value" text-selection>
                    Proposal:{" "}
                    {
                      proposalOptions.find(
                        ([value]) => value === taskOf(v)?.proposal,
                      )?.[1]
                    }
                  </text>
                  <text className="conflict-value" text-selection>
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
                    pressed={selectedHeadId === v.versionId}
                    disabled={resolutionStaleNow || busy}
                    onClick={() => {
                      if (submitting.current) return;
                      setResolution(v);
                      setSelectedHeadId(v.versionId);
                    }}
                  >
                    Use this version
                  </Button>
                </view>
              ))}
            </view>
          </view>
          {!resolutionStaleNow && retained && taskOf(retained) && (
            <view className="card conflict-retained">
              <text>Retained resolution from an earlier review</text>
              <text text-selection>Title: {taskOf(retained)?.title}</text>
              <text text-selection>
                Description: {taskOf(retained)?.description}
              </text>
              <text text-selection>
                Prompt:{" "}
                {taskOf(retained)?.prompt === undefined
                  ? "Not set"
                  : taskOf(retained)?.prompt === ""
                    ? "Empty"
                    : taskOf(retained)?.prompt}
              </text>
              <Button
                disabled={busy}
                onClick={() => {
                  if (submitting.current) return;
                  setResolution(retained);
                  setSelectedHeadId(undefined);
                }}
              >
                Use retained resolution
              </Button>
            </view>
          )}
          <view className="conflict-resolution">
            <text className="heading" accessibility-heading>
              Your resolution
            </text>
            <text className="muted">
              {selectedHeadId
                ? currentHeads.some((v) => v.versionId === selectedHeadId)
                  ? `Started from Version ${currentHeads.findIndex((v) => v.versionId === selectedHeadId) + 1}`
                  : "Started from a previous version"
                : resolution
                  ? "Retained resolution from an earlier review"
                  : "Choose a current version or explicitly use the retained resolution."}
            </text>
            {resolution && taskOf(resolution) && (
              <>
                <Field
                  label="Resolved title"
                  value={taskOf(resolution)!.title}
                  disabled={busy}
                  onChange={(title) => editResolution({ title })}
                />
                <Field
                  label="Resolved description"
                  multiline
                  value={taskOf(resolution)!.description}
                  disabled={busy}
                  onChange={(description) => editResolution({ description })}
                />
                <Choices
                  label="Resolved prompt presence"
                  exclusive
                  disabled={busy}
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
                    editResolution({
                      prompt: promptForPresence(
                        taskOf(resolution)!.prompt,
                        value,
                      ),
                    })
                  }
                />
                <text className="muted">
                  Typing into Resolved prompt includes it. An included empty
                  prompt is different from No prompt.
                </text>
                <Field
                  label="Resolved prompt"
                  multiline
                  value={taskOf(resolution)!.prompt || ""}
                  disabled={busy}
                  onChange={(prompt) => editResolution({ prompt })}
                />
                <view className="conflict-status-groups">
                  <Choices
                    label="Resolved progress"
                    disabled={busy}
                    value={taskOf(resolution)!.progress}
                    options={progressOptions}
                    onChange={(progress) => editResolution({ progress })}
                  />
                  <Choices
                    label="Resolved proposal"
                    disabled={busy}
                    value={taskOf(resolution)!.proposal}
                    options={proposalOptions}
                    onChange={(proposal) => editResolution({ proposal })}
                  />
                </view>
                <Choices
                  label="Resolved repository"
                  disabled={busy}
                  value={taskOf(resolution)!.repositoryId}
                  options={repositories.map(
                    (r) =>
                      [r.version!.entityId, repositoryName(r.version)] as const,
                  )}
                  onChange={(repositoryId) => editResolution({ repositoryId })}
                />
              </>
            )}
          </view>
          <view className="conflict-footer">
            <text className="muted">
              Save resolution creates a new whole-task version.
            </text>
            <Button
              primary
              disabled={
                busy || !resolution || !reviewedHeads || resolutionStaleNow
              }
              onClick={() =>
                void run(async (completion) => {
                  if (!reviewedHeads || resolutionStale(reviewedHeads, current))
                    throw new Error(
                      "The task changed after you started this resolution. Review the current versions before saving.",
                    );
                  const r = await adapter.call(
                    LaneService.method.resolveConflict,
                    {
                      meta: meta(),
                      id: draft.id,
                      expectedHeadIds: reviewedHeads,
                      resolved: resolution,
                    },
                  );
                  if (!r.result || r.result.result === ResultKind.CONFLICT)
                    throw new Error(
                      "The conflict changed. Your resolution is retained. Review the current versions before resolving again.",
                    );
                  setResolve(false);
                  setReviewedHeads(undefined);
                  setRetained(undefined);
                  setResolution(undefined);
                  setSelectedHeadId(undefined);
                  completion.saved(r.result);
                })
              }
            >
              {busy ? "Saving…" : "Save resolution"}
            </Button>
            {busy && <Notice>Saving resolution…</Notice>}
            {error && (
              <view
                className="notice conflict-error"
                accessibility-element
                accessibility-traits="text"
              >
                <text>{error}</text>
              </view>
            )}
          </view>
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
