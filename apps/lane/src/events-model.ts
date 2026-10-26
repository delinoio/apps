// SPDX-License-Identifier: Apache-2.0
import {
  WebhookService,
  EventState,
  type WebhookReceiver as ProtoWebhookReceiver,
  type WebhookEvent as ProtoWebhookEvent,
  type ProcessingRecord as ProtoProcessingRecord,
} from "@repo/protos/lane/v1/lane_pb";
import type { LaneAdapter } from "./adapter.ts";
import type { UI } from "./serializable.ts";

export type WebhookReceiver = UI<ProtoWebhookReceiver>;
export type WebhookEvent = UI<ProtoWebhookEvent>;
export type ProcessingRecord = UI<ProtoProcessingRecord>;

// The Events filters that define which receivers and events a load returns.
export interface EventsQuery {
  repositoryId: string;
  state: EventState;
}

export function eventsQueryKey(query: EventsQuery): string {
  return `${query.repositoryId}\u0000${query.state}`;
}

export interface EventsView {
  receivers: WebhookReceiver[];
  events: WebhookEvent[];
  // Continuation cursor for `cursorKey` only; empty when no continuation exists.
  cursor: string;
  // Query key that owns `cursor`. Continuation is refused for any other query.
  cursorKey: string;
  // The first page of the current query is still pending.
  loading: boolean;
  // A continuation page of the current query is still pending.
  loadingMore: boolean;
  // Failure of the current query's most recent load; empty otherwise.
  error: string;
}

export const emptyEventsView: EventsView = {
  receivers: [],
  events: [],
  cursor: "",
  cursorKey: "",
  loading: false,
  loadingMore: false,
  error: "",
};

const PAGE_SIZE = 100;

// The event the details dialog shows, with its processing history.
export interface EventDetail {
  event: WebhookEvent;
  processing: ProcessingRecord[];
}

export interface EventDetailView {
  // The accepted detail shown in the dialog; undefined while closed.
  selected: EventDetail | undefined;
  // The current inspection is still pending.
  loading: boolean;
  // Failure of the current inspection; empty otherwise.
  error: string;
}

export const emptyEventDetailView: EventDetailView = {
  selected: undefined,
  loading: false,
  error: "",
};

// Owns Events detail reads. Every inspection is a separate intent with its own
// generation number, even when it targets an ID that is already pending. The
// newest inspection, closing the detail and disposal invalidate older reads, so
// an obsolete response can neither open or replace the detail nor publish its
// error. Reads never modify events. Mutation outcomes are not routed here.
export class EventDetailReader {
  private generation = 0;
  private disposed = false;
  private view: EventDetailView = emptyEventDetailView;

  constructor(
    private readonly adapter: LaneAdapter,
    private readonly publish: (view: EventDetailView) => void,
  ) {}

  // Starts reading `id`, superseding every older inspection. The accepted
  // detail stays visible until this read, if still current, replaces it.
  async inspect(id: string): Promise<void> {
    if (this.disposed) return;
    const generation = ++this.generation;
    this.set({ loading: true, error: "" });
    try {
      const r = await this.adapter.call(WebhookService.method.getEvent, {
        id,
      });
      if (!this.owns(generation)) return;
      // A missing event leaves the current detail state unchanged.
      if (r.event)
        this.set({ selected: { event: r.event, processing: r.processing } });
    } catch (e) {
      if (this.owns(generation))
        this.set({
          error:
            e instanceof Error
              ? e.message
              : "Operation failed. Input is retained.",
        });
    } finally {
      if (this.owns(generation)) this.set({ loading: false });
    }
  }

  // Replaces the accepted event after a mutation, keeping its history. Ignored
  // while no detail is open and when `event` is not the accepted event, so a
  // delayed mutation response for an event whose detail was closed cannot
  // replace the detail of a different event.
  update(event: WebhookEvent): void {
    if (
      this.disposed ||
      !this.view.selected ||
      this.view.selected.event.id !== event.id
    )
      return;
    this.set({ selected: { ...this.view.selected, event } });
  }

  // Closes the detail and ends the ownership of every pending inspection.
  close(): void {
    if (this.disposed) return;
    this.generation++;
    this.set({ selected: undefined, loading: false, error: "" });
  }

  dispose(): void {
    this.disposed = true;
    this.generation++;
  }

  private owns(generation: number): boolean {
    return !this.disposed && generation === this.generation;
  }

  private set(patch: Partial<EventDetailView>): void {
    this.view = { ...this.view, ...patch };
    this.publish(this.view);
  }
}

// Owns Events list loading. Every load takes a generation number; the last
// started reload, any query change, and disposal invalidate older work so an
// obsolete load can neither publish data, cursor or status nor continue its
// receiver page chain. Mutation outcomes are not routed through this object.
export class EventsLoader {
  private generation = 0;
  private disposed = false;
  private query: EventsQuery | undefined;
  private pending = false;
  private view: EventsView = emptyEventsView;

  constructor(
    private readonly adapter: LaneAdapter,
    private readonly publish: (view: EventsView) => void,
  ) {}

  // Starts the first page of `query`, superseding all older work. A different
  // query drops the previous query's rows and cursor immediately.
  reload(query: EventsQuery): Promise<void> {
    if (this.disposed) return Promise.resolve();
    const changed =
      !this.query || eventsQueryKey(this.query) !== eventsQueryKey(query);
    this.query = query;
    return this.start(changed);
  }

  // Reloads the latest requested query. Callbacks from older renders use this
  // so they cannot revive an obsolete filter selection.
  refresh(): Promise<void> {
    if (this.disposed || !this.query) return Promise.resolve();
    return this.start(false);
  }

  // Loads the next event page for `query`. Ignored unless `query` is the
  // current query, its first page succeeded and no other load is in flight.
  loadMore(query: EventsQuery): Promise<void> {
    const key = eventsQueryKey(query);
    if (
      this.disposed ||
      this.pending ||
      !this.query ||
      eventsQueryKey(this.query) !== key ||
      !this.view.cursor ||
      this.view.cursorKey !== key
    )
      return Promise.resolve();
    const generation = ++this.generation;
    const { cursor } = this.view;
    this.pending = true;
    this.set({ loadingMore: true, error: "" });
    return this.runOwned(generation, async () => {
      const r = await this.listEvents(query, cursor);
      if (!this.owns(generation)) return;
      this.set({
        events: [...this.view.events, ...r.items],
        cursor: r.nextCursor,
        cursorKey: r.nextCursor ? key : "",
      });
    });
  }

  dispose(): void {
    this.disposed = true;
    this.generation++;
    this.pending = false;
  }

  private start(changed: boolean): Promise<void> {
    const query = this.query!;
    const key = eventsQueryKey(query);
    const generation = ++this.generation;
    this.pending = true;
    this.set({
      ...(changed ? { receivers: [], events: [] } : {}),
      // A new first page always invalidates any earlier continuation.
      cursor: "",
      cursorKey: "",
      loading: true,
      loadingMore: false,
      error: "",
    });
    return this.runOwned(generation, async () => {
      let receiverCursor = "";
      const all: WebhookReceiver[] = [];
      do {
        const r = await this.adapter.call(WebhookService.method.listReceivers, {
          page: { size: PAGE_SIZE, cursor: receiverCursor },
          repositoryId: query.repositoryId || undefined,
        });
        if (!this.owns(generation)) return;
        all.push(...r.items);
        receiverCursor = r.nextCursor;
      } while (receiverCursor);
      this.set({ receivers: all });
      const r = await this.listEvents(query, "");
      if (!this.owns(generation)) return;
      this.set({
        events: r.items,
        cursor: r.nextCursor,
        cursorKey: r.nextCursor ? key : "",
      });
    });
  }

  private listEvents(query: EventsQuery, cursor: string) {
    return this.adapter.call(WebhookService.method.listEvents, {
      page: { size: PAGE_SIZE, cursor },
      repositoryId: query.repositoryId || undefined,
      states: query.state ? [query.state] : [],
    });
  }

  private async runOwned(
    generation: number,
    body: () => Promise<void>,
  ): Promise<void> {
    try {
      await body();
    } catch (e) {
      if (this.owns(generation))
        this.set({
          error:
            e instanceof Error
              ? e.message
              : "Operation failed. Input is retained.",
        });
    } finally {
      if (this.owns(generation)) {
        this.pending = false;
        this.set({ loading: false, loadingMore: false });
      }
    }
  }

  private owns(generation: number): boolean {
    return !this.disposed && generation === this.generation;
  }

  private set(patch: Partial<EventsView>): void {
    this.view = { ...this.view, ...patch };
    this.publish(this.view);
  }
}
