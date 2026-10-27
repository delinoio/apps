// SPDX-License-Identifier: Apache-2.0
import type { UI } from "./serializable";
import { useEffect, useRef, useState } from "@lynx-js/react";
import {
  WebhookService,
  EventState,
  type EntityResult as ProtoEntityResult,
} from "@repo/protos/lane/v1/lane_pb";
import { meta, newId, type LaneAdapter } from "./adapter";
import { Button, Choices, Dialog, Field, Notice } from "./components";
import {
  EventDetailReader,
  EventsLoader,
  emptyEventDetailView,
  emptyEventsView,
  eventsQueryKey,
  type EventDetailView,
  type EventsView,
} from "./events-model";
import { repositoryName } from "./model";
import { hostAction } from "./runtime";
export function Events({
  adapter,
  repositories,
  revision,
  openTask,
  stateChanged,
  active = true,
}: {
  adapter: LaneAdapter;
  repositories: EntityResult[];
  // Server-change invalidation counter owned by App. A new value rereads the
  // first page of the active query; Events keeps its identity and local state.
  revision: number;
  openTask: (id: string) => void;
  active?: boolean;
  stateChanged?: (dirty: boolean, busy: boolean) => void;
}) {
  const [view, setView] = useState<EventsView>(emptyEventsView);
  const { receivers, events, cursor, cursorKey } = view;
  const [repositoryId, setRepositoryId] = useState("");
  const [state, setState] = useState<EventState>(EventState.UNSPECIFIED);
  const [editing, setEditing] = useState<{
    id: string;
    revision?: string;
    repositoryId: string;
    name: string;
  }>();
  const [detail, setDetail] = useState<EventDetailView>(emptyEventDetailView);
  const { selected } = detail;
  const [secret, setSecret] = useState<{ secret: string; endpoint?: string }>();
  const [deleting, setDeleting] = useState<{
    kind: "receiver" | "event";
    id: string;
  }>();
  const [mutating, setMutating] = useState(false);
  // Tracks only the receiver editor's own save. It is separate from the shared
  // `busy` flag so unrelated reloads never lock the receiver fields while the
  // user is typing. The ref closes the window before the next render disables
  // the controls.
  const [savingReceiver, setSavingReceiver] = useState(false);
  const savingReceiverRef = useRef(false);
  const [error, setError] = useState("");
  // A pending current inspection also blocks mutations, as before; obsolete
  // inspections never reach this state.
  const busy = mutating || detail.loading;
  useEffect(() => {
    stateChanged?.(!!editing, mutating || savingReceiver);
  }, [editing, mutating, savingReceiver]);
  const run = async (fn: () => Promise<void>) => {
    setMutating(true);
    setError("");
    try {
      await fn();
    } catch (e) {
      setError(
        e instanceof Error ? e.message : "Operation failed. Input is retained.",
      );
    } finally {
      setMutating(false);
    }
  };
  // Owns list loading so only the latest load for the current adapter and
  // filters can publish rows, cursor and load status. Mutation outcomes stay in
  // run() above.
  const loader = useRef<EventsLoader | undefined>(undefined);
  const query = { repositoryId, state };
  useEffect(() => {
    const owned = new EventsLoader(adapter, setView);
    loader.current = owned;
    setView(emptyEventsView);
    return () => {
      owned.dispose();
      if (loader.current === owned) loader.current = undefined;
    };
  }, [adapter]);
  // Reloading the same query keeps filters, rows and dialogs, restarts the
  // first page and supersedes older loads and continuations.
  useEffect(() => {
    void loader.current?.reload({ repositoryId, state });
  }, [adapter, repositoryId, state, revision]);
  // Owns detail reads so only the newest inspection of the active adapter can
  // open or replace the dialog or publish its status; Close, Escape, leaving to
  // a linked task and unmounting end the ownership of older reads.
  const reader = useRef<EventDetailReader | undefined>(undefined);
  useEffect(() => {
    const owned = new EventDetailReader(adapter, setDetail);
    reader.current = owned;
    setDetail(emptyEventDetailView);
    return () => {
      owned.dispose();
      if (reader.current === owned) reader.current = undefined;
    };
  }, [adapter]);
  useEffect(() => {
    // A tab retains its list and receiver draft, but an inspection requested
    // before leaving must not open a surprise dialog when the tab returns.
    if (!active) reader.current?.close();
  }, [active]);
  const closeDetail = () => reader.current?.close();
  const load = async () => {
    await loader.current?.refresh();
  };
  return (
    <view className="section">
      <text className="heading">Events and webhook receivers</text>
      <Notice>
        Lane retains events. Your existing agent claims and processes them
        through MCP; reading does not complete an event.
      </Notice>
      <Choices
        label="Repository"
        value={repositoryId}
        options={[
          ["", "All repositories"],
          ...repositories.map(
            (r) => [r.version!.entityId, repositoryName(r.version)] as const,
          ),
        ]}
        onChange={setRepositoryId}
      />
      <Choices
        label="Event state"
        value={state}
        options={[
          [EventState.UNSPECIFIED, "All"],
          [EventState.PENDING, "Pending"],
          [EventState.LEASED, "Processing"],
          [EventState.COMPLETED, "Completed"],
        ]}
        onChange={setState}
      />
      <view className="row wrap">
        <Button onClick={() => void load()}>Refresh events</Button>
        <Button
          primary
          onClick={() =>
            setEditing({
              id: newId(),
              repositoryId:
                repositoryId || repositories[0]?.version?.entityId || "",
              name: "",
            })
          }
        >
          Create receiver
        </Button>
      </view>
      {(error || view.error || detail.error) && (
        <Notice>{error || view.error || detail.error}</Notice>
      )}
      <text className="heading">Receiving settings</text>
      {receivers.map((r) => (
        <view className="card" key={r.id}>
          <text>{r.name}</text>
          <text>
            Repository:{" "}
            {repositories.find((x) => x.version?.entityId === r.repositoryId)
              ? repositoryName(
                  repositories.find(
                    (x) => x.version?.entityId === r.repositoryId,
                  )?.version,
                )
              : r.repositoryId}
          </text>
          <view className="row wrap">
            <Button
              onClick={() =>
                setEditing({
                  id: r.id,
                  revision: r.revision,
                  repositoryId: r.repositoryId,
                  name: r.name,
                })
              }
            >
              Edit receiver
            </Button>
            <Button
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  const result = await adapter.call(
                    WebhookService.method.rotateReceiverKey,
                    { meta: meta(), id: r.id, expectedRevision: r.revision },
                  );
                  setSecret({ secret: result.secret });
                  await load();
                })
              }
            >
              Rotate signing key
            </Button>
            <Button onClick={() => setDeleting({ kind: "receiver", id: r.id })}>
              Delete receiver
            </Button>
          </view>
        </view>
      ))}
      <text className="heading">Retained events</text>
      {events.length === 0 && view.loading && (
        <text className="muted">Loading events...</text>
      )}
      {events.length === 0 && !view.loading && (
        <text className="muted">No events match these filters.</text>
      )}
      {events.map((e) => (
        <view className="card" key={e.id}>
          <text>
            {e.eventType} · {EventState[e.state]}
          </text>
          <text>Sender event: {e.senderEventId}</text>
          <Button
            onClick={() => {
              setError("");
              void reader.current?.inspect(e.id);
            }}
          >
            Inspect event
          </Button>
        </view>
      ))}
      {cursor && cursorKey === eventsQueryKey(query) && (
        <Button
          disabled={view.loading || view.loadingMore}
          onClick={() => void loader.current?.loadMore(query)}
        >
          Load more events
        </Button>
      )}
      {editing && (
        <Dialog title="Webhook receiver" onClose={() => setEditing(undefined)}>
          <Field
            label="Receiver name"
            value={editing.name}
            disabled={savingReceiver}
            onChange={(name) => {
              if (savingReceiverRef.current) return;
              setEditing({ ...editing, name });
            }}
            focus
          />
          <Choices
            label="Repository"
            value={editing.repositoryId}
            options={repositories.map(
              (r) => [r.version!.entityId, repositoryName(r.version)] as const,
            )}
            disabled={savingReceiver}
            onChange={(repositoryId) => {
              if (savingReceiverRef.current) return;
              setEditing({ ...editing, repositoryId });
            }}
          />
          <Notice>
            Changing the repository applies only to future events. Existing
            events and their linked task IDs stay in their original repository.
          </Notice>
          {error && <Notice>{error}</Notice>}
          <Button
            primary
            disabled={
              busy ||
              savingReceiver ||
              !editing.repositoryId ||
              !editing.name.trim()
            }
            onClick={() => {
              if (savingReceiverRef.current) return;
              savingReceiverRef.current = true;
              setSavingReceiver(true);
              void run(async () => {
                // The lock covers the mutation only. A rejected mutation
                // keeps the submitted draft editable for an explicit retry;
                // the follow-up reload does not hold the new editor state.
                try {
                  if (editing.revision !== undefined)
                    await adapter.call(WebhookService.method.saveReceiver, {
                      meta: meta(),
                      id: editing.id,
                      expectedRevision: editing.revision,
                      repositoryId: editing.repositoryId,
                      name: editing.name,
                    });
                  else {
                    const r = await adapter.call(
                      WebhookService.method.createReceiver,
                      {
                        meta: meta(),
                        id: editing.id,
                        repositoryId: editing.repositoryId,
                        name: editing.name,
                      },
                    );
                    setSecret({ secret: r.secret, endpoint: r.endpoint });
                  }
                  setEditing(undefined);
                } finally {
                  savingReceiverRef.current = false;
                  setSavingReceiver(false);
                }
                await load();
              });
            }}
          >
            Save
          </Button>
        </Dialog>
      )}
      {secret && (
        <Dialog
          title="Save this signing key now"
          onClose={() => setSecret(undefined)}
        >
          <Notice>
            This key is shown once. Closing this screen discards it. A rotated
            key invalidates the old key immediately. Lost keys require rotation.
          </Notice>
          {secret.endpoint && (
            <text text-selection>Endpoint: {secret.endpoint}</text>
          )}
          <text text-selection>{secret.secret}</text>
          <Button
            onClick={() =>
              void run(() => hostAction("copyText", secret.secret))
            }
          >
            Copy signing key
          </Button>
          <text>
            Send the Lane common format with event ID, type, occurrence time and
            JSON object payload. Sign the exact body and timestamp. Retries keep
            the event ID and use a fresh signature.
          </text>
        </Dialog>
      )}
      {selected && (
        <Dialog title="Event details" onClose={closeDetail}>
          <text>
            {selected.event.eventType} · {EventState[selected.event.state]}
          </text>
          <text text-selection>
            {JSON.stringify(selected.event.payload, null, 2)}
          </text>
          <text className="heading">Processing history</text>
          {selected.processing.map((p) => (
            <view className="card" key={p.id}>
              <text>
                {p.origin?.deviceName} ·{" "}
                {p.completedAt
                  ? new Date(
                      Number(p.completedAt.seconds) * 1000,
                    ).toLocaleString()
                  : ""}
              </text>
              {p.taskIds.length === 0 && (
                <text>Completed without creating tasks.</text>
              )}
              {p.taskIds.map((id) => (
                <Button
                  key={id}
                  onClick={() => {
                    closeDetail();
                    openTask(id);
                  }}
                >
                  Open linked task {id}
                </Button>
              ))}
            </view>
          ))}
          <Button
            disabled={busy || selected.event.state !== EventState.COMPLETED}
            onClick={() =>
              void run(async () => {
                const r = await adapter.call(
                  WebhookService.method.reopenEvent,
                  {
                    meta: meta(),
                    id: selected.event.id,
                    expectedRevision: selected.event.revision,
                  },
                );
                if (r.event) reader.current?.update(r.event);
                await load();
              })
            }
          >
            Reopen for processing
          </Button>
          <Button
            onClick={() =>
              setDeleting({ kind: "event", id: selected.event.id })
            }
          >
            Delete event
          </Button>
          {(error || detail.error) && <Notice>{error || detail.error}</Notice>}
        </Dialog>
      )}
      {deleting && (
        <Dialog
          title={`Delete ${deleting.kind}?`}
          onClose={() => setDeleting(undefined)}
        >
          <Notice>
            {deleting.kind === "receiver"
              ? "Stop new intake. Retained events and existing processing leases remain."
              : "Permanently remove this event and invalidate its lease. Replaying the same sender event will not recreate it."}
          </Notice>
          <Button
            disabled={busy}
            onClick={() =>
              void run(async () => {
                if (deleting.kind === "receiver")
                  await adapter.call(WebhookService.method.deleteReceiver, {
                    meta: meta(),
                    id: deleting.id,
                  });
                else
                  await adapter.call(WebhookService.method.deleteEvent, {
                    meta: meta(),
                    id: deleting.id,
                  });
                setDeleting(undefined);
                closeDetail();
                await load();
              })
            }
          >
            Delete permanently
          </Button>
          {error && <Notice>{error}</Notice>}
        </Dialog>
      )}
    </view>
  );
}

type EntityResult = UI<ProtoEntityResult>;
