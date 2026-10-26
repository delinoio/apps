// SPDX-License-Identifier: Apache-2.0
import type { UI } from "./serializable";
import { useState } from "@lynx-js/react";
import {
  LaneService,
  ResultKind,
  type EntityResult as ProtoEntityResult,
  type EntityVersion as ProtoEntityVersion,
} from "@repo/protos/lane/v1/lane_pb";
import { type LaneAdapter, meta, newId } from "./adapter";
import { Button, Dialog, Field, Notice } from "./components";
import { conflicted, heads, repositoryName } from "./model";
export function Repositories({
  adapter,
  items,
  refresh,
  select,
}: {
  adapter: LaneAdapter;
  items: EntityResult[];
  refresh: () => void;
  select: (id: string) => void;
}) {
  const [editing, setEditing] = useState<{
    id: string;
    base?: string;
    url: string;
    name: string;
  }>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [deleting, setDeleting] = useState<EntityVersion>();
  const [conflict, setConflict] = useState<EntityResult>();
  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    setError("");
    try {
      await fn();
      refresh();
    } catch (e) {
      setError(
        e instanceof Error
          ? e.message
          : "Operation failed. Your input is retained.",
      );
    } finally {
      setBusy(false);
    }
  };
  const current = items.find((i) => i.version?.entityId === editing?.id);
  return (
    <view className="section">
      <text className="heading">Repositories</text>
      <text className="muted">
        Organize tasks by repository URL. Lane does not fetch repository
        contents.
      </text>
      <Button
        primary
        onClick={() => setEditing({ id: newId(), url: "", name: "" })}
      >
        Add repository
      </Button>
      {items.map((r) => (
        <view className="card" key={r.version?.entityId}>
          <text className="heading">{repositoryName(r.version)}</text>
          <text>
            {r.version?.body.case === "repository"
              ? r.version.body.value.normalizedUrl
              : ""}
          </text>
          <view className="row wrap">
            <Button onClick={() => select(r.version!.entityId)}>
              View tasks
            </Button>
            <Button
              disabled={conflicted(r)}
              onClick={() => {
                const v = r.version!;
                setEditing({
                  id: v.entityId,
                  base: v.versionId,
                  url:
                    v.body.case === "repository"
                      ? `https://${v.body.value.normalizedUrl}`
                      : "",
                  name:
                    v.body.case === "repository"
                      ? v.body.value.displayName || ""
                      : "",
                });
              }}
            >
              Rename or change URL
            </Button>
            {conflicted(r) && (
              <Button onClick={() => setConflict(r)}>
                Resolve repository conflict
              </Button>
            )}
            <Button onClick={() => setDeleting(r.version)}>
              Delete repository
            </Button>
          </view>
        </view>
      ))}
      {editing && (
        <Dialog
          title={editing.base ? "Edit repository" : "Add repository"}
          onClose={() => setEditing(undefined)}
        >
          {current?.version?.versionId !== editing.base && editing.base && (
            <Notice>
              External changes arrived. Your input is retained; Save checks your
              original version.
            </Notice>
          )}
          <Field
            label="Repository URL"
            value={editing.url}
            onChange={(url) => setEditing({ ...editing, url })}
            focus
          />
          <Field
            label="Display name (optional)"
            value={editing.name}
            onChange={(name) => setEditing({ ...editing, name })}
          />
          <Notice>
            Changing the URL keeps tasks, history and receivers. The old URL
            will not remain an alias.
          </Notice>
          {error && <Notice>{error}</Notice>}
          <Button
            primary
            disabled={busy || conflicted(current)}
            onClick={() =>
              void run(async () => {
                const input = {
                  meta: meta(),
                  id: editing.id,
                  url: editing.url,
                  displayName: editing.name || undefined,
                };
                const response = editing.base
                  ? await adapter.call(LaneService.method.saveRepository, {
                      ...input,
                      expectedVersionId: editing.base,
                    })
                  : await adapter.call(
                      LaneService.method.createRepository,
                      input,
                    );
                if (response.result?.result === ResultKind.CONFLICT) {
                  refresh();
                  throw new Error(
                    "Repository versions conflict. Your input is retained. Close this editor to compare and resolve the saved alternatives.",
                  );
                }
                setEditing(undefined);
              })
            }
          >
            Save
          </Button>
        </Dialog>
      )}
      {deleting && (
        <Dialog
          title="Permanently delete repository?"
          onClose={() => setDeleting(undefined)}
        >
          <Notice>
            {repositoryName(deleting)}: delete its current tasks, prompts,
            histories and retained events. Tasks already moved elsewhere remain.
            There is no recovery.
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
        >
          {heads(conflict).map((v) => (
            <view className="card" key={v.versionId}>
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
