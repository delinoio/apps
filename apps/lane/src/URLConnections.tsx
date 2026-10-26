// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "@lynx-js/react";
import {
  CaptureService,
  LaneService,
  ResultKind,
  type EntityResult,
  type EntityVersion,
} from "@delino/protos/lane/v1/lane_pb";
import type { UI } from "./serializable";
import { meta, newId, type LaneAdapter, type LocalStatus } from "./adapter";
import { Button, Choices, Dialog, Field, Notice } from "./components";
import { conflicted, heads, repositoryName } from "./model";
export const connectionOf = (version?: UI<EntityVersion>) =>
  version?.body.case === "urlConnection" ? version.body.value : undefined;
export function URLConnections({
  adapter,
  repositories,
  dirty,
}: {
  adapter: LaneAdapter;
  repositories: UI<EntityResult>[];
  dirty: (value: boolean) => void;
}) {
  const [items, setItems] = useState<UI<EntityResult>[]>([]);
  const [status, setStatus] = useState<LocalStatus>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const lock = useRef(false);
  const [revision, setRevision] = useState(0);
  const [editing, setEditing] = useState<{
    id: string;
    base?: string;
    prefix: string;
    repositoryId: string;
    requestId: string;
  }>();
  const [deleting, setDeleting] = useState<string>();
  const [conflict, setConflict] = useState<UI<EntityResult>>();
  const [chosen, setChosen] = useState<UI<EntityVersion>>();
  const [discard, setDiscard] = useState(false);
  const [discardConflict, setDiscardConflict] = useState(false);
  useEffect(() => {
    dirty(!!editing || !!chosen);
    return () => dirty(false);
  }, [editing, chosen, dirty]);
  useEffect(
    () => adapter.subscribe(() => setRevision((v) => v + 1)),
    [adapter],
  );
  useEffect(() => {
    let active = true;
    void (async () => {
      const all: UI<EntityResult>[] = [];
      let cursor = "";
      do {
        const result = await adapter.call(
          CaptureService.method.listURLConnections,
          { page: { size: 100, cursor } },
        );
        all.push(...result.items);
        cursor = result.nextCursor;
      } while (cursor);
      const local = await adapter.local("GetLocalStatus", {});
      if (active) {
        setItems(all);
        setStatus(local);
        setError("");
      }
    })().catch((e) => {
      if (active) setError(e.message);
    });
    return () => {
      active = false;
    };
  }, [adapter, revision]);
  const run = async (action: () => Promise<void>) => {
    if (lock.current) return;
    lock.current = true;
    setBusy(true);
    setError("");
    try {
      await action();
      setRevision((v) => v + 1);
    } catch (e) {
      setError(
        e instanceof Error
          ? e.message
          : "Operation failed. Your input is retained.",
      );
    } finally {
      lock.current = false;
      setBusy(false);
    }
  };
  const options = repositories
    .filter((r) => r.version && !r.version.deleted && !conflicted(r))
    .map((r) => [r.version!.entityId, repositoryName(r.version)] as const);
  const syncLabel = !status
    ? "Checking synchronization…"
    : !status.syncEnabled
      ? "Local only"
      : status.syncErrorReason || status.itemErrors?.length
        ? "Synchronization failed"
        : status.pendingItemCount !== "0"
          ? "Synchronization pending"
          : "Synchronized";
  return (
    <view className="section">
      <text className="heading" accessibility-heading>
        URL connections
      </text>
      <text className="muted">{syncLabel}</text>
      <text>
        Use the longest matching URL prefix to select a repository for the first
        capture. Conflicting rules are excluded.
      </text>
      <Button
        onClick={() =>
          setEditing({
            id: newId(),
            prefix: "",
            repositoryId: "",
            requestId: newId(),
          })
        }
      >
        Add connection
      </Button>
      {!items.length && <text>No URL connections</text>}
      {items.map((item) => (
        <view className="card" key={item.version?.entityId}>
          <text text-selection>{connectionOf(item.version)?.prefix}</text>
          <text>
            {repositoryName(
              repositories.find(
                (r) =>
                  r.version?.entityId ===
                  connectionOf(item.version)?.repositoryId,
              )?.version,
            )}
          </text>
          <view className="row wrap">
            <Button
              disabled={busy || conflicted(item)}
              onClick={() =>
                setEditing({
                  id: item.version!.entityId,
                  base: item.version!.versionId,
                  prefix: connectionOf(item.version)!.prefix,
                  repositoryId: connectionOf(item.version)!.repositoryId,
                  requestId: newId(),
                })
              }
            >
              Edit
            </Button>
            <Button
              disabled={busy}
              onClick={() => setDeleting(item.version!.entityId)}
            >
              Delete
            </Button>
            {conflicted(item) && (
              <Button
                onClick={() => {
                  setConflict(item);
                  setChosen(undefined);
                }}
              >
                Resolve conflict
              </Button>
            )}
          </view>
        </view>
      ))}
      {error && <Notice>{error}</Notice>}
      {error && !editing && (
        <Button onClick={() => setRevision((v) => v + 1)}>Retry</Button>
      )}
      {editing && (
        <Dialog
          title={editing.base ? "Edit URL connection" : "Add URL connection"}
          onClose={() => setDiscard(true)}
        >
          <Field
            label="URL prefix"
            value={editing.prefix}
            focus
            onChange={(prefix) =>
              setEditing({ ...editing, prefix, requestId: newId() })
            }
          />
          {!editing.prefix.trim() && (
            <text>Enter an HTTP or HTTPS URL prefix.</text>
          )}
          <Choices
            label="Repository"
            value={editing.repositoryId}
            options={options}
            onChange={(repositoryId) =>
              setEditing({ ...editing, repositoryId, requestId: newId() })
            }
          />
          {!options.some(([id]) => id === editing.repositoryId) && (
            <Notice>Select an available repository.</Notice>
          )}
          {error && <Notice>{error}</Notice>}
          <Button
            disabled={busy}
            primary
            onClick={() =>
              void run(async () => {
                if (
                  !editing.prefix.trim() ||
                  !options.some(([id]) => id === editing.repositoryId)
                )
                  throw new Error(
                    "Enter a URL prefix and select a repository.",
                  );
                const connection = {
                  prefix: editing.prefix,
                  repositoryId: editing.repositoryId,
                };
                const reply = editing.base
                  ? await adapter.call(
                      CaptureService.method.saveURLConnection,
                      {
                        meta: { requestId: editing.requestId },
                        id: editing.id,
                        expectedVersionId: editing.base,
                        connection,
                      },
                    )
                  : await adapter.call(
                      CaptureService.method.createURLConnection,
                      {
                        meta: { requestId: editing.requestId },
                        id: editing.id,
                        connection,
                      },
                    );
                if (
                  !reply.result ||
                  reply.result.result === ResultKind.CONFLICT
                )
                  throw new Error(
                    "The rule changed. Close this editor and resolve both alternatives.",
                  );
                setEditing(undefined);
              })
            }
          >
            {busy ? "Saving…" : error ? "Retry" : "Save"}
          </Button>
        </Dialog>
      )}
      {discard && (
        <Dialog
          title="Discard URL connection changes?"
          onClose={() => setDiscard(false)}
        >
          <Button onClick={() => setDiscard(false)}>Keep editing</Button>
          <Button
            onClick={() => {
              setEditing(undefined);
              setDiscard(false);
            }}
          >
            Discard changes
          </Button>
        </Dialog>
      )}
      {discardConflict && (
        <Dialog
          title="Discard conflict resolution?"
          onClose={() => setDiscardConflict(false)}
        >
          <Button primary onClick={() => setDiscardConflict(false)}>
            Keep editing
          </Button>
          <Button
            onClick={() => {
              setChosen(undefined);
              setConflict(undefined);
              setDiscardConflict(false);
            }}
          >
            Discard resolution
          </Button>
        </Dialog>
      )}
      {deleting && (
        <Dialog
          title="Delete URL connection?"
          onClose={() => setDeleting(undefined)}
        >
          <text>This removes the matching rule. Saved tasks are retained.</text>
          <Button
            disabled={busy}
            onClick={() =>
              void run(async () => {
                await adapter.call(CaptureService.method.deleteURLConnection, {
                  meta: meta(),
                  id: deleting,
                });
                setDeleting(undefined);
              })
            }
          >
            Delete connection
          </Button>
          {error && <Notice>{error}</Notice>}
        </Dialog>
      )}
      {conflict && (
        <Dialog
          title="Resolve URL connection conflict"
          onClose={() => {
            if (chosen) setDiscardConflict(true);
            else setConflict(undefined);
          }}
        >
          <Notice>
            Compare both alternatives. Select one, check its repository, then
            explicitly save the resolution.
          </Notice>
          {heads(conflict).map((version) => (
            <view className="card" key={version.versionId}>
              <text text-selection>{connectionOf(version)?.prefix}</text>
              <text>
                {repositoryName(
                  repositories.find(
                    (r) =>
                      r.version?.entityId ===
                      connectionOf(version)?.repositoryId,
                  )?.version,
                )}
              </text>
              <Button
                primary={chosen?.versionId === version.versionId}
                onClick={() => setChosen(version)}
              >
                Use this version
              </Button>
            </view>
          ))}
          {chosen && (
            <Choices
              label="Resolved repository"
              value={connectionOf(chosen)!.repositoryId}
              options={options}
              onChange={(repositoryId) =>
                setChosen({
                  ...chosen,
                  body: {
                    case: "urlConnection",
                    value: { ...connectionOf(chosen)!, repositoryId },
                  },
                })
              }
            />
          )}
          {error && <Notice>{error}</Notice>}
          <Button
            primary
            disabled={busy || !chosen}
            onClick={() =>
              void run(async () => {
                if (
                  !chosen ||
                  !options.some(
                    ([id]) => id === connectionOf(chosen)?.repositoryId,
                  )
                )
                  throw new Error("Select an available repository.");
                const reply = await adapter.call(
                  LaneService.method.resolveConflict,
                  {
                    meta: meta(),
                    id: chosen.entityId,
                    expectedHeadIds: heads(conflict)
                      .map((v) => v.versionId)
                      .sort(),
                    resolved: chosen,
                  },
                );
                if (
                  !reply.result ||
                  reply.result.result === ResultKind.CONFLICT
                )
                  throw new Error(
                    "The conflict changed. Refresh and compare all alternatives.",
                  );
                setChosen(undefined);
                setConflict(undefined);
              })
            }
          >
            Save resolution
          </Button>
        </Dialog>
      )}
    </view>
  );
}
