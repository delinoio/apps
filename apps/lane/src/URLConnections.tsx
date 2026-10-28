// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "@lynx-js/react";
import {
  CaptureService,
  LaneService,
  ResultKind,
  type EntityResult,
  type EntityVersion,
} from "@repo/protos/lane/v1/lane_pb";
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
  const [loaded, setLoaded] = useState(false);
  const [loading, setLoading] = useState(true);
  const [status, setStatus] = useState<LocalStatus>();
  const [loadError, setLoadError] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const lock = useRef(false);
  const [revision, setRevision] = useState(0);
  const [editing, setEditing] = useState<{
    sessionId: string;
    id: string;
    base?: string;
    prefix: string;
    repositoryId: string;
    requestId: string;
  }>();
  const [deleting, setDeleting] = useState<string>();
  const [resolution, setResolution] = useState<{
    conflict: UI<EntityResult>;
    chosen?: UI<EntityVersion>;
    requestId: string;
  }>();
  const resolutionRef = useRef(resolution);
  resolutionRef.current = resolution;
  const [discard, setDiscard] = useState(false);
  const [discardConflict, setDiscardConflict] = useState(false);
  useEffect(() => {
    dirty(!!editing || !!resolution?.chosen);
    return () => dirty(false);
  }, [editing, resolution, dirty]);
  useEffect(
    () => adapter.subscribe(() => setRevision((v) => v + 1)),
    [adapter],
  );
  useEffect(() => {
    let active = true;
    setLoading(true);
    setStatus(undefined);
    setLoadError("");
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
      if (active) {
        setItems(all);
        setLoaded(true);
      }
      const local = await adapter.local("GetLocalStatus", {});
      if (active) {
        setStatus(local);
        setLoadError("");
      }
    })()
      .catch((e) => {
        if (active) setLoadError(e.message);
      })
      .finally(() => {
        if (active) setLoading(false);
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
  const displayRepository = (id?: string) => {
    const repository = repositories.find(
      (item) => item.version?.entityId === id,
    );
    return repository?.version &&
      !repository.version.deleted &&
      !conflicted(repository)
      ? repositoryName(repository.version)
      : "Repository unavailable";
  };
  const syncLabel = !status
    ? loading || !loadError
      ? "Checking synchronization…"
      : "Synchronization status unavailable"
    : !status.syncEnabled
      ? "Local only"
      : status.syncErrorReason || status.itemErrors?.length
        ? "Synchronization failed"
        : status.pendingItemCount !== "0"
          ? "Synchronization pending"
          : "Synchronized";
  return (
    <view className="realqa-section realqa-url-section">
      <view className="realqa-section-header">
        <view className="realqa-section-copy">
          <text className="heading" accessibility-heading>
            URL connections
          </text>
          <text className="muted">{syncLabel}</text>
        </view>
        <Button
          disabled={busy}
          onClick={() =>
            setEditing({
              sessionId: newId(),
              id: newId(),
              prefix: "",
              repositoryId: "",
              requestId: newId(),
            })
          }
        >
          Add connection
        </Button>
      </view>
      <text>
        Use the longest matching URL prefix to select a repository for the first
        capture. Conflicting rules are excluded.
      </text>
      {error && <Notice>{error}</Notice>}
      {loading && !loaded && <text>Loading URL connections…</text>}
      {!loading && !loaded && loadError && (
        <text>Could not load URL connections.</text>
      )}
      {loaded && !items.length && <text>No URL connections</text>}
      {items.length > 0 && (
        <view className="realqa-group">
          {items.map((item) => (
            <view className="realqa-setting-row" key={item.version?.entityId}>
              <view className="realqa-row-value">
                {heads(item).map((version) => (
                  <view key={version.versionId} className="realqa-rule-version">
                    <text className="realqa-long-value" text-selection>
                      {connectionOf(version)?.prefix}
                    </text>
                    <text className="muted realqa-long-value">
                      {displayRepository(connectionOf(version)?.repositoryId)}
                    </text>
                  </view>
                ))}
                {conflicted(item) && <text>Conflicting alternatives</text>}
              </view>
              <view className="realqa-row-actions">
                <Button
                  disabled={busy || conflicted(item)}
                  onClick={() =>
                    setEditing({
                      sessionId: newId(),
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
                    disabled={busy}
                    onClick={() => {
                      setResolution({
                        conflict: item,
                        requestId: newId(),
                      });
                    }}
                  >
                    Resolve conflict
                  </Button>
                )}
              </view>
            </view>
          ))}
        </view>
      )}
      {loadError && <Notice>{loadError}</Notice>}
      {loadError && (
        <Button onClick={() => setRevision((v) => v + 1)}>Retry</Button>
      )}
      {editing && (
        <Dialog
          className="realqa-dialog"
          title={editing.base ? "Edit URL connection" : "Add URL connection"}
          onClose={() => setDiscard(true)}
        >
          <Field
            label="URL prefix"
            value={editing.prefix}
            focus
            disabled={busy}
            error={
              !editing.prefix.trim()
                ? "Enter an HTTP or HTTPS URL prefix."
                : /url|prefix|duplicate|http/i.test(error)
                  ? error
                  : undefined
            }
            onChange={(prefix) => {
              if (lock.current) return;
              setEditing((current) =>
                current?.sessionId === editing.sessionId &&
                current.prefix !== prefix
                  ? { ...current, prefix, requestId: newId() }
                  : current,
              );
            }}
          />
          <Choices
            label="Repository"
            value={editing.repositoryId}
            options={options}
            disabled={busy}
            onChange={(repositoryId) => {
              if (lock.current) return;
              setEditing((current) =>
                current?.sessionId === editing.sessionId &&
                current.repositoryId !== repositoryId
                  ? { ...current, repositoryId, requestId: newId() }
                  : current,
              );
            }}
          />
          {!options.some(([id]) => id === editing.repositoryId) && (
            <text className="validation">Select an available repository.</text>
          )}
          {error && <Notice>{error}</Notice>}
          {loadError && (
            <Button onClick={() => setRevision((v) => v + 1)}>
              Retry loading
            </Button>
          )}
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
                // An input accepted before the busy transition can still be
                // queued when this reply arrives. Clear only its submitted draft.
                setEditing((current) => {
                  if (current?.sessionId !== editing.sessionId) return current;
                  if (current.requestId === editing.requestId) return undefined;
                  const savedVersionId = reply.result?.version?.versionId;
                  return savedVersionId
                    ? { ...current, base: savedVersionId }
                    : current;
                });
              })
            }
          >
            {busy ? "Saving…" : error ? "Retry" : "Save"}
          </Button>
        </Dialog>
      )}
      {discard && (
        <Dialog
          className="realqa-dialog"
          title="Discard URL connection changes?"
          onClose={() => setDiscard(false)}
        >
          <Button focusOnShow onClick={() => setDiscard(false)}>
            Keep editing
          </Button>
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
          className="realqa-dialog"
          title="Discard conflict resolution?"
          onClose={() => setDiscardConflict(false)}
        >
          <Button primary focusOnShow onClick={() => setDiscardConflict(false)}>
            Keep editing
          </Button>
          <Button
            onClick={() => {
              setResolution(undefined);
              setDiscardConflict(false);
            }}
          >
            Discard resolution
          </Button>
        </Dialog>
      )}
      {deleting && (
        <Dialog
          className="realqa-dialog"
          title="Delete URL connection?"
          onClose={() => setDeleting(undefined)}
        >
          <text>This removes the matching rule. Saved tasks are retained.</text>
          <Button focusOnShow onClick={() => setDeleting(undefined)}>
            Cancel
          </Button>
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
          {loadError && (
            <Button onClick={() => setRevision((v) => v + 1)}>
              Retry loading
            </Button>
          )}
        </Dialog>
      )}
      {resolution && (
        <Dialog
          className="realqa-dialog"
          title="Resolve URL connection conflict"
          onClose={() => {
            if (resolution.chosen) setDiscardConflict(true);
            else setResolution(undefined);
          }}
        >
          <Notice>
            Compare both alternatives. Select one, check its repository, then
            explicitly save the resolution.
          </Notice>
          {heads(resolution.conflict).map((version) => (
            <view className="realqa-conflict-choice" key={version.versionId}>
              <text className="realqa-long-value" text-selection>
                {connectionOf(version)?.prefix}
              </text>
              <text className="realqa-long-value">
                {displayRepository(connectionOf(version)?.repositoryId)}
              </text>
              <Button
                primary={resolution.chosen?.versionId === version.versionId}
                disabled={busy}
                onClick={() => {
                  if (lock.current) return;
                  setResolution((current) =>
                    current?.conflict === resolution.conflict &&
                    current.chosen !== version
                      ? { ...current, chosen: version, requestId: newId() }
                      : current,
                  );
                }}
              >
                Use this version
              </Button>
            </view>
          ))}
          {resolution.chosen && (
            <Choices
              label="Resolved repository"
              value={connectionOf(resolution.chosen)!.repositoryId}
              options={options}
              disabled={busy}
              onChange={(repositoryId) => {
                if (lock.current) return;
                setResolution((current) =>
                  current?.conflict === resolution.conflict &&
                  current.chosen &&
                  connectionOf(current.chosen)?.repositoryId !== repositoryId
                    ? {
                        ...current,
                        chosen: {
                          ...current.chosen,
                          body: {
                            case: "urlConnection",
                            value: {
                              ...connectionOf(current.chosen)!,
                              repositoryId,
                            },
                          },
                        },
                        requestId: newId(),
                      }
                    : current,
                );
              }}
            />
          )}
          {resolution.chosen &&
            !options.some(
              ([id]) => id === connectionOf(resolution.chosen)?.repositoryId,
            ) && (
              <text className="validation">
                Select an available repository.
              </text>
            )}
          {error && <Notice>{error}</Notice>}
          {loadError && (
            <Button onClick={() => setRevision((v) => v + 1)}>
              Retry loading
            </Button>
          )}
          <Button
            primary
            disabled={busy || !resolution.chosen}
            onClick={() =>
              void run(async () => {
                if (
                  !resolution.chosen ||
                  !options.some(
                    ([id]) =>
                      id === connectionOf(resolution.chosen)?.repositoryId,
                  )
                )
                  throw new Error("Select an available repository.");
                const reply = await adapter.call(
                  LaneService.method.resolveConflict,
                  {
                    meta: { requestId: resolution.requestId },
                    id: resolution.chosen.entityId,
                    expectedHeadIds: heads(resolution.conflict)
                      .map((v) => v.versionId)
                      .sort(),
                    resolved: resolution.chosen,
                  },
                );
                if (
                  !reply.result ||
                  reply.result.result === ResultKind.CONFLICT
                )
                  throw new Error(
                    "The conflict changed. Refresh and compare all alternatives.",
                  );
                const current = resolutionRef.current;
                const savedVersion = reply.result.version;
                const retained = current?.chosen
                  ? connectionOf(current.chosen)
                  : undefined;
                const canRebase =
                  !!current &&
                  current !== resolution &&
                  !!retained &&
                  !!savedVersion;
                if (
                  current &&
                  current !== resolution &&
                  retained &&
                  savedVersion
                ) {
                  setEditing({
                    sessionId: newId(),
                    id: savedVersion.entityId,
                    base: savedVersion.versionId,
                    prefix: retained.prefix,
                    repositoryId: retained.repositoryId,
                    requestId: current.requestId,
                  });
                }
                setResolution((active) =>
                  active === resolution || canRebase ? undefined : active,
                );
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
