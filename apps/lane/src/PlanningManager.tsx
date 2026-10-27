// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "@lynx-js/react";
import type { UI } from "./serializable";
import {
  LaneService,
  PlanningService,
  ResultKind,
  type EntityResult as ProtoEntityResult,
  type EntityVersion as ProtoEntityVersion,
} from "@repo/protos/lane/v1/lane_pb";
import { meta, newId, type LaneAdapter } from "./adapter";
import { Button, Choices, Dialog, Field, Notice } from "./components";
import { heads, repositoryName } from "./model";
import {
  PlanningKind,
  blankPlanningInput,
  describeError,
  hasErrors,
  itemsOf,
  planningBody,
  planningInputChanged,
  planningInputOf,
  planningItemChanges,
  planningItemOf,
  planningKinds,
  validatePlanningInput,
  type PlanningInput,
  type PlanningItem,
} from "./planning";
import type { PlanningState } from "./usePlanning";

type EntityResult = UI<ProtoEntityResult>;
type EntityVersion = UI<ProtoEntityVersion>;
interface Editing {
  id: string;
  base?: string;
  input: PlanningInput;
  // Input at selection or last successful save; differences are unsaved edits.
  initial: PlanningInput;
}
const noun: Record<PlanningKind, string> = {
  [PlanningKind.Label]: "label",
  [PlanningKind.Milestone]: "milestone",
  [PlanningKind.Cycle]: "cycle",
};
const describe = (item: PlanningItem) =>
  item.kind === PlanningKind.Label
    ? item.color
    : item.kind === PlanningKind.Milestone
      ? `${item.completed ? "Completed" : "Open"}${item.targetDate ? ` · Target ${item.targetDate}` : ""}`
      : `${item.startDate} to ${item.endDate}`;

export function PlanningManager({
  adapter,
  repository,
  planning,
  refresh,
  dirty,
  busyChanged,
  back,
}: {
  adapter: LaneAdapter;
  repository: EntityResult;
  planning: PlanningState;
  refresh: () => void;
  dirty: (value: boolean) => void;
  busyChanged?: (value: boolean) => void;
  back: () => void;
}) {
  const repositoryId = repository.version!.entityId;
  const [kind, setKind] = useState(PlanningKind.Label);
  const [editing, setEditing] = useState<Editing>();
  const [attempted, setAttempted] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    busyChanged?.(busy);
  }, [busy, busyChanged]);
  const [deleting, setDeleting] = useState(false);
  const [history, setHistory] = useState<EntityVersion[] | null>(null);
  const [historyCursor, setHistoryCursor] = useState("");
  const [conflict, setConflict] = useState<EntityResult>();
  const [discard, setDiscard] = useState<() => void>();
  const lock = useRef(false);
  // Keep the request ID across an unchanged retry, including response loss.
  const attempt = useRef<{ key: string; requestId: string } | undefined>(
    undefined,
  );
  const siblings = itemsOf(planning.items, kind, repositoryId);
  const current = editing
    ? planning.items.find((i) => i.id === editing.id)
    : undefined;
  const isNew = !!editing && !editing.base;
  const missing = !!editing?.base && planning.status === "ready" && !current;
  const changedElsewhere =
    !!editing?.base && !!current && current.versionId !== editing.base;
  const readOnly = !!current?.conflicted || missing;
  const errors =
    editing && attempted
      ? validatePlanningInput(kind, editing.input, siblings, editing.id)
      : {};
  const run = async (action: () => Promise<void>) => {
    if (lock.current) return;
    lock.current = true;
    setBusy(true);
    setError("");
    try {
      await action();
    } catch (e) {
      setError(describeError(e));
    } finally {
      lock.current = false;
      setBusy(false);
    }
  };
  const unsaved =
    !!editing && planningInputChanged(editing.initial, editing.input);
  // Report unsaved input to the app so page and app navigation ask first.
  useEffect(() => {
    dirty(unsaved);
    return () => dirty(false);
  }, [unsaved, dirty]);
  // Every action that replaces or closes the editor asks before dropping unsaved input.
  const guard = (action: () => void) => {
    if (unsaved) setDiscard(() => action);
    else action();
  };
  const start = (item?: PlanningItem) =>
    guard(() => {
      setAttempted(false);
      setError("");
      const input = item ? planningInputOf(item) : blankPlanningInput();
      setEditing({
        id: item ? item.id : newId(),
        base: item?.versionId,
        input,
        initial: input,
      });
    });
  const patch = (changes: Partial<PlanningInput>) =>
    editing &&
    setEditing({ ...editing, input: { ...editing.input, ...changes } });
  const save = () =>
    run(async () => {
      if (!editing) return;
      setAttempted(true);
      if (
        hasErrors(
          validatePlanningInput(kind, editing.input, siblings, editing.id),
        )
      )
        throw new Error("Fix the fields marked above, then Save again.");
      const item = planningBody(kind, repositoryId, editing.input);
      const key = JSON.stringify({ id: editing.id, base: editing.base, item });
      const requestId =
        attempt.current?.key === key ? attempt.current.requestId : newId();
      attempt.current = { key, requestId };
      const response = editing.base
        ? await adapter.call(PlanningService.method.savePlanningItem, {
            meta: { requestId },
            id: editing.id,
            expectedVersionId: editing.base,
            item,
          })
        : await adapter.call(PlanningService.method.createPlanningItem, {
            meta: { requestId },
            id: editing.id,
            item,
          });
      if (!response.result)
        throw new Error(
          "The server returned no item. Your input is retained; refresh before retrying.",
        );
      refresh();
      if (response.result.result === ResultKind.CONFLICT)
        throw new Error(
          "Both versions were preserved. Resolve the conflict, then edit again.",
        );
      // A created item continues as an edit of its first version. Tabs and item
      // selection stay enabled during a slow save, so apply the response only if the
      // submitted item is still the open editor; otherwise keep the user's selection.
      const submitted = editing;
      const base = response.result.version?.versionId;
      setEditing((open) =>
        open && open.id === submitted.id
          ? { ...open, base, initial: submitted.input }
          : open,
      );
    });
  const loadHistory = async (more = false) => {
    if (!editing) return;
    const r = await adapter.call(LaneService.method.listHistory, {
      id: editing.id,
      page: { size: 100, cursor: more ? historyCursor : "" },
    });
    setHistory(more ? [...(history || []), ...r.versions] : r.versions);
    setHistoryCursor(r.nextCursor);
  };
  const tab = planningKinds.find(([k]) => k === kind)!;
  const list = itemsOf(planning.items, kind, repositoryId);
  return (
    <view className="section">
      <view className="row between wrap">
        <view>
          <text className="heading">
            {repositoryName(repository.version)}: Labels, milestones and cycles
          </text>
          <text className="muted">
            Planning items belong to this repository. Tasks reference them by
            identity.
          </text>
        </view>
        <Button onClick={() => guard(back)}>Back to repositories</Button>
      </view>
      <view className="row wrap planning-tabs">
        {planningKinds.map(([k, title]) => (
          <Button
            key={k}
            pressed={k === kind}
            label={`${title} tab`}
            onClick={() =>
              guard(() => {
                setKind(k);
                setEditing(undefined);
                setError("");
              })
            }
          >
            {`${title} (${itemsOf(planning.items, k, repositoryId).length})`}
          </Button>
        ))}
      </view>
      {planning.status === "error" && (
        <Notice>
          Could not load planning items. {planning.error}{" "}
          <Button onClick={planning.retry}>Retry loading</Button>
        </Notice>
      )}
      <view className="planning-manager">
        <view className="planning-list">
          <view className="row">
            <Button primary onClick={() => start()}>
              {`New ${noun[kind]}`}
            </Button>
          </view>
          {planning.status === "loading" && <text>Loading…</text>}
          {planning.status === "ready" && list.length === 0 && (
            <view className="empty">
              <text className="heading">{`No ${tab[1].toLowerCase()} yet`}</text>
              <text>
                {`Create the first ${noun[kind]} for this repository, then assign it to tasks.`}
              </text>
              <Button primary onClick={() => start()}>
                {`Create ${noun[kind]}`}
              </Button>
            </view>
          )}
          {list.map((item) => (
            <view className="planning-row" key={item.id}>
              <Button
                pressed={editing?.id === item.id}
                swatch={
                  item.kind === PlanningKind.Label ? item.color : undefined
                }
                label={`Edit ${noun[kind]} ${item.name}`}
                onClick={() => start(item)}
              >
                {`${item.name} · ${describe(item)}`}
              </Button>
              {item.conflicted && <text className="badge">Conflict</text>}
            </view>
          ))}
        </view>
        <view className="planning-edit">
          {!editing ? (
            <text className="muted">
              Select an item to edit it, or create a new one.
            </text>
          ) : (
            <view key={editing.id}>
              <text className="heading">
                {isNew ? `New ${noun[kind]}` : `Edit ${noun[kind]}`}
              </text>
              {changedElsewhere && (
                <Notice>
                  This {noun[kind]} changed elsewhere. Your input is retained;
                  Save checks your original version.
                </Notice>
              )}
              {missing && (
                <Notice>
                  This {noun[kind]} was deleted. Your unsaved input stays here
                  for copying; it cannot be saved.
                </Notice>
              )}
              {current?.conflicted && (
                <Notice>
                  This {noun[kind]} has a conflict. Editing is paused until you
                  compare and resolve the versions.{" "}
                  <Button
                    onClick={() =>
                      void run(async () => {
                        const r = await adapter.call(
                          PlanningService.method.getPlanningItem,
                          { id: editing.id },
                        );
                        setConflict(r.result);
                      })
                    }
                  >
                    Compare conflict
                  </Button>
                </Notice>
              )}
              <Field
                label="Name"
                value={editing.input.name}
                onChange={(name) => patch({ name })}
                disabled={busy || readOnly}
                error={errors.name}
                focus
              />
              {kind === PlanningKind.Label && (
                <Field
                  label="Color (#RRGGBB)"
                  value={editing.input.color}
                  onChange={(color) => patch({ color: color.trim() })}
                  disabled={busy || readOnly}
                  error={errors.color}
                  help="The label name is always shown next to its color."
                />
              )}
              {kind === PlanningKind.Milestone && (
                <>
                  <Field
                    label="Target date (YYYY-MM-DD, optional)"
                    value={editing.input.targetDate}
                    onChange={(targetDate) =>
                      patch({ targetDate: targetDate.trim() })
                    }
                    disabled={busy || readOnly}
                    error={errors.targetDate}
                  />
                  <Choices
                    exclusive
                    label="Completion"
                    value={editing.input.completed ? "done" : "open"}
                    options={[
                      ["open", "Open"],
                      ["done", "Completed"],
                    ]}
                    onChange={(value) => patch({ completed: value === "done" })}
                    disabled={busy || readOnly}
                  />
                  <text className="muted">
                    Completing a milestone does not change task progress.
                  </text>
                </>
              )}
              {kind === PlanningKind.Cycle && (
                <view className="field-grid">
                  <Field
                    label="Start date (YYYY-MM-DD)"
                    value={editing.input.startDate}
                    onChange={(startDate) =>
                      patch({ startDate: startDate.trim() })
                    }
                    disabled={busy || readOnly}
                    error={errors.startDate}
                  />
                  <Field
                    label="End date (YYYY-MM-DD)"
                    value={editing.input.endDate}
                    onChange={(endDate) => patch({ endDate: endDate.trim() })}
                    disabled={busy || readOnly}
                    error={errors.endDate}
                  />
                </view>
              )}
              <Field
                label="Description"
                multiline
                value={editing.input.description}
                onChange={(description) => patch({ description })}
                disabled={busy || readOnly}
                error={errors.description}
              />
              {error && <Notice>{error} Your input is retained.</Notice>}
              <view className="row wrap">
                <Button
                  primary
                  disabled={busy || readOnly}
                  onClick={() => void save()}
                >
                  {busy ? "Saving…" : error ? "Retry Save" : "Save"}
                </Button>
                {!isNew && (
                  <>
                    <Button
                      disabled={busy}
                      onClick={() => void run(() => loadHistory())}
                    >
                      History
                    </Button>
                    <Button
                      disabled={busy || missing}
                      onClick={() => setDeleting(true)}
                    >
                      Delete
                    </Button>
                  </>
                )}
                <Button onClick={() => guard(() => setEditing(undefined))}>
                  Close
                </Button>
              </view>
            </view>
          )}
        </view>
      </view>
      {discard && (
        <Dialog
          title="Discard unsaved changes?"
          onClose={() => setDiscard(undefined)}
        >
          <Notice>Your edits have not been saved.</Notice>
          <Button
            onClick={() => {
              const action = discard;
              setDiscard(undefined);
              action();
            }}
          >
            Discard changes
          </Button>
          <Button primary onClick={() => setDiscard(undefined)}>
            Keep editing
          </Button>
        </Dialog>
      )}
      {deleting && editing && (
        <Dialog
          title={`Delete ${noun[kind]}?`}
          onClose={() => setDeleting(false)}
        >
          <Notice>
            Deleting “{editing.input.name}” detaches it from every task that
            uses it. The tasks remain. The {noun[kind]} and its history are
            permanently removed. There is no recovery.
          </Notice>
          {error && <Notice>{error}</Notice>}
          <Button
            disabled={busy}
            onClick={() =>
              void run(async () => {
                await adapter.call(PlanningService.method.deletePlanningItem, {
                  meta: meta(),
                  id: editing.id,
                });
                setDeleting(false);
                setEditing(undefined);
                refresh();
              })
            }
          >
            {busy ? "Deleting…" : "Delete permanently"}
          </Button>
          <Button primary onClick={() => setDeleting(false)}>
            Keep {noun[kind]}
          </Button>
        </Dialog>
      )}
      {history && (
        <Dialog title="Item history" onClose={() => setHistory(null)}>
          <text className="muted">
            Before and after values, time and origin. History is read-only and
            has no restore action.
          </text>
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
              {planningItemChanges(history[index - 1], version).map((c) => (
                <view className="history-change" key={c.field}>
                  <text className="label">{c.field}</text>
                  <text text-selection>Before: {c.before}</text>
                  <text text-selection>After: {c.after}</text>
                </view>
              ))}
            </view>
          ))}
          {historyCursor && (
            <Button onClick={() => void run(() => loadHistory(true))}>
              Load more history
            </Button>
          )}
          {error && <Notice>{error}</Notice>}
        </Dialog>
      )}
      {conflict && (
        <Dialog
          title={`Compare ${noun[kind]} versions`}
          onClose={() => setConflict(undefined)}
        >
          {heads(conflict).map((v) => {
            const item = planningItemOf({
              ...conflict,
              version: v,
              alternatives: [],
            });
            return (
              <view className="card" key={v.versionId}>
                <text text-selection>{item?.name}</text>
                <text text-selection>{item ? describe(item) : ""}</text>
                <text text-selection>{item?.description}</text>
                <text className="muted">{v.origin?.deviceName}</text>
                <Button
                  disabled={busy}
                  onClick={() =>
                    void run(async () => {
                      await adapter.call(LaneService.method.resolveConflict, {
                        meta: meta(),
                        id: v.entityId,
                        expectedHeadIds: heads(conflict)
                          .map((h) => h.versionId)
                          .sort(),
                        resolved: v,
                      });
                      setConflict(undefined);
                      setEditing(undefined);
                      refresh();
                    })
                  }
                >
                  Keep this version
                </Button>
              </view>
            );
          })}
          {error && (
            <Notice>
              {error} Refresh the list before trying a stale resolution again.
            </Notice>
          )}
        </Dialog>
      )}
    </view>
  );
}
