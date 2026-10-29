// SPDX-License-Identifier: Apache-2.0
import type { UI } from "./serializable";
import { useCallback, useEffect, useState } from "@lynx-js/react";
import {
  LaneService,
  type EntityResult as ProtoEntityResult,
  type EntityVersion as ProtoEntityVersion,
} from "@repo/protos/lane/v1/lane_pb";
import { type LaneAdapter, meta, newId } from "./adapter";
import {
  RepositoryEditor,
  RepositorySaveOutcome,
  repositoryConflictMessage,
  repositoryDraft,
} from "./repositoryEditor";
import { Button, Dialog, Field, Notice } from "./components";
import { conflicted, heads, repositoryName } from "./model";
import { PlanningManager } from "./PlanningManager";
import type { PlanningState } from "./usePlanning";
export function Repositories({
  adapter,
  items,
  status,
  loadError,
  planning,
  refresh,
  select,
  dirty,
  busyChanged,
}: {
  adapter: LaneAdapter;
  items: EntityResult[];
  status: string;
  loadError: string;
  planning: PlanningState;
  refresh: () => void;
  select: (id: string) => void;
  dirty: (value: boolean) => void;
  busyChanged?: (value: boolean) => void;
}) {
  const [detail, setDetail] = useState("");
  // The editor owns the draft, its lifetime and its retry identity; re-render on change.
  const [, setRevision] = useState(0);
  const [editor] = useState(
    () => new RepositoryEditor(() => setRevision((value) => value + 1)),
  );
  const editing = editor.draft;
  const [error, setError] = useState("");
  const [pendingOperations, setPendingOperations] = useState(0);
  const busy = pendingOperations > 0;
  const [planningDirty, setPlanningDirty] = useState(false);
  const [planningOperations, setPlanningOperations] = useState(0);
  // The repository pane outlives its planning manager. Retain each operation
  // until its promise settles, including operations from an older manager.
  const beginPlanningOperation = useCallback(() => {
    setPlanningOperations((count) => count + 1);
    let finished = false;
    return () => {
      if (finished) return;
      finished = true;
      setPlanningOperations((count) => count - 1);
    };
  }, []);
  const planningBusy = planningOperations > 0;
  const stored = editing?.base
    ? items.find((item) => item.version?.entityId === editing.id)?.version
    : undefined;
  const original = stored ? repositoryDraft(stored) : undefined;
  const editorDirty =
    !!editing &&
    (original
      ? editing.url !== original.url || editing.name !== original.name
      : !!editing.url || !!editing.name);
  useEffect(() => {
    dirty(editorDirty || planningDirty);
  }, [editorDirty, planningDirty, dirty]);
  useEffect(() => {
    busyChanged?.(busy || planningBusy);
  }, [busy, planningBusy, busyChanged]);
  const [deleting, setDeleting] = useState<EntityVersion>();
  const [conflict, setConflict] = useState<EntityResult>();
  const current = items.find((i) => i.version?.entityId === editing?.id);
  const unavailable =
    !!editing?.base && !current && status !== "Loading repositories...";
  const errorMessage = (e: unknown) =>
    e instanceof Error
      ? e.message
      : "Operation failed. Your input is retained.";
  const run = async (fn: () => Promise<void>) => {
    setPendingOperations((count) => count + 1);
    setError("");
    try {
      await fn();
      refresh();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setPendingOperations((count) => count - 1);
    }
  };
  const save = async () => {
    if (!editing || editor.saving || unavailable || conflicted(current)) return;
    setPendingOperations((count) => count + 1);
    setError("");
    try {
      const result = await editor.save(adapter);
      if (result.outcome === RepositorySaveOutcome.Ignored) return;
      // The mutation happened regardless of the editor lifetime, so lists always refresh.
      if (result.outcome !== RepositorySaveOutcome.Failed) refresh();
      // Only the editor that submitted the save reports into it.
      if (!result.current) return;
      if (result.outcome === RepositorySaveOutcome.Conflict)
        setError(repositoryConflictMessage);
      else if (result.outcome === RepositorySaveOutcome.Failed)
        setError(errorMessage(result.error));
    } finally {
      setPendingOperations((count) => count - 1);
    }
  };
  const detailRepository = items.find((i) => i.version?.entityId === detail);
  if (detailRepository)
    return (
      <PlanningManager
        adapter={adapter}
        repository={detailRepository}
        planning={planning}
        refresh={refresh}
        dirty={setPlanningDirty}
        beginOperation={beginPlanningOperation}
        back={() => setDetail("")}
      />
    );
  return (
    <view className="section repository-directory">
      <view className="repository-header">
        <view className="repository-header-copy">
          <text className="heading">Repositories</text>
          <text className="muted">
            Organize tasks by repository URL. Lane does not fetch repository
            contents.
          </text>
        </view>
        <Button
          primary
          onClick={() => {
            setError("");
            editor.open({ id: newId(), url: "", name: "" });
          }}
        >
          Add repository
        </Button>
      </view>
      {status === "Loading repositories..." && (
        <view
          className="repository-status notice"
          accessibility-element
          accessibility-traits="text"
        >
          <text>Loading repositories...</text>
          {!items.length && (
            <view
              className="repository-placeholder"
              accessibility-elements-hidden
            />
          )}
        </view>
      )}
      {status === "Repositories could not be refreshed." && (
        <view
          className="repository-status notice"
          accessibility-element
          accessibility-traits="text"
        >
          <text>
            {items.length
              ? "Repositories could not be refreshed. Existing rows remain available."
              : "Repositories are unavailable. Your current input is retained."}
          </text>
          {!!loadError && <text className="muted">{loadError}</text>}
          <Button onClick={refresh}>Retry loading repositories</Button>
        </view>
      )}
      {!status && items.length === 0 && (
        <view className="repository-empty">
          <text className="heading">No repositories yet</text>
          <text className="muted">
            Add a repository URL to organize tasks. Lane will not fetch its
            contents.
          </text>
          <Button
            onClick={() => {
              setError("");
              editor.open({ id: newId(), url: "", name: "" });
            }}
          >
            Add repository
          </Button>
        </view>
      )}
      {items.map((r) => (
        <view className="repository-row" key={r.version?.entityId}>
          <view className="repository-identity">
            <text className="repository-name">{repositoryName(r.version)}</text>
            <text className="repository-url">
              {r.version?.body.case === "repository"
                ? r.version.body.value.normalizedUrl
                : ""}
            </text>
          </view>
          <view className="repository-actions">
            <Button onClick={() => select(r.version!.entityId)}>
              View tasks
            </Button>
            <Button onClick={() => setDetail(r.version!.entityId)}>
              Labels, milestones and cycles
            </Button>
            <Button
              disabled={conflicted(r)}
              onClick={() => {
                const v = r.version!;
                setError("");
                editor.open(repositoryDraft(v));
              }}
            >
              Rename or change URL
            </Button>
            {conflicted(r) && (
              <Button
                onClick={() => {
                  setError("");
                  setConflict(r);
                }}
              >
                Resolve repository conflict
              </Button>
            )}
            <Button
              onClick={() => {
                setError("");
                setDeleting(r.version);
              }}
            >
              Delete repository
            </Button>
          </view>
        </view>
      ))}
      {editing && (
        <Dialog
          title={editing.base ? "Edit repository" : "Add repository"}
          className="repository-dialog"
          onClose={() => editor.close()}
        >
          {editing.base && (
            <text className="muted">
              Update the name or URL used to organize your tasks.
            </text>
          )}
          {current &&
            current.version?.versionId !== editing.base &&
            editing.base && (
              <Notice>
                External changes arrived. Your input is retained; Save checks
                your original version.
              </Notice>
            )}
          <Field
            label="Repository URL"
            value={editing.url}
            onChange={(url) => editor.edit({ url })}
            disabled={editor.saving}
            focus
            focusOnShow
            help="Equivalent HTTPS and SSH forms identify the same repository."
            accessibleHelp
            error={
              error && /url|invalid|required|duplicate/i.test(error)
                ? error
                : undefined
            }
          />
          <Field
            label="Display name (optional)"
            value={editing.name}
            onChange={(name) => editor.edit({ name })}
            disabled={editor.saving}
          />
          <Notice>
            Changing the URL keeps tasks, history and receivers. The old URL
            will not remain an alias.
          </Notice>
          {unavailable && (
            <Notice>
              This repository is unavailable. Refresh the list before saving;
              your input is retained.
            </Notice>
          )}
          {conflicted(current) && (
            <Notice>
              This repository has conflicting versions. Close this editor to
              compare and resolve them.
            </Notice>
          )}
          {error && <Notice>{error}</Notice>}
          {editor.saving && <Notice>Saving repository...</Notice>}
          <view className="repository-dialog-footer">
            <Button
              primary
              disabled={editor.saving || unavailable || conflicted(current)}
              onClick={() => void save()}
            >
              {editor.saving ? "Saving..." : "Save"}
            </Button>
          </view>
        </Dialog>
      )}
      {deleting && (
        <Dialog
          title="Permanently delete repository?"
          onClose={() => setDeleting(undefined)}
          className="repository-dialog"
          closeFocus
        >
          <Notice>
            {repositoryName(deleting)}: delete its current tasks, prompts,
            histories, labels, milestones, cycles and retained events. Tasks
            already moved elsewhere remain. There is no recovery.
          </Notice>
          {error && <Notice>{error}</Notice>}
          <Button
            disabled={busy}
            onClick={() =>
              void run(async () => {
                await adapter.call(LaneService.method.deleteRepository, {
                  meta: meta(),
                  id: deleting.entityId,
                });
                setDeleting(undefined);
              })
            }
          >
            Delete permanently
          </Button>
        </Dialog>
      )}
      {conflict && (
        <Dialog
          title="Compare repository versions"
          onClose={() => setConflict(undefined)}
          className="repository-dialog"
        >
          {heads(conflict).map((v) => (
            <view
              className="card"
              key={v.versionId}
              accessibility-label={`${repositoryName(v)}, ${v.body.case === "repository" ? v.body.value.normalizedUrl : ""}, ${v.origin?.deviceName || "Unknown device"}`}
            >
              <text>{repositoryName(v)}</text>
              <text>
                {v.body.case === "repository" ? v.body.value.normalizedUrl : ""}
              </text>
              <text>{v.origin?.deviceName}</text>
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
                  })
                }
              >
                Keep this name and URL
              </Button>
            </view>
          ))}
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

type EntityResult = UI<ProtoEntityResult>;

type EntityVersion = UI<ProtoEntityVersion>;
