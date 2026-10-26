// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { EventState, WebhookService } from "@repo/protos/lane/v1/lane_pb";
import type { LaneAdapter } from "../src/adapter.ts";
import {
  EventDetailReader,
  EventsLoader,
  emptyEventDetailView,
  type EventDetailView,
  eventsQueryKey,
  emptyEventsView,
  type EventsQuery,
  type EventsView,
} from "../src/events-model.ts";

interface Call {
  kind: "receivers" | "events";
  input: {
    repositoryId?: string;
    page: { cursor: string };
    states?: EventState[];
  };
  resolve(value: { items: string[]; nextCursor: string }): void;
  reject(error: Error): void;
}

// Deferred read-only adapter: every request waits until the test settles it.
function harness() {
  const calls: Call[] = [];
  const adapter = {
    call(method: unknown, input: Call["input"]) {
      const kind =
        method === WebhookService.method.listReceivers ? "receivers" : "events";
      return new Promise((resolve, reject) =>
        calls.push({
          kind,
          input,
          resolve: resolve as Call["resolve"],
          reject,
        }),
      );
    },
  } as unknown as LaneAdapter;
  let view: EventsView = emptyEventsView;
  const loader = new EventsLoader(adapter, (v) => (view = v));
  return { calls, loader, view: () => view };
}

const tick = () => new Promise((resolve) => setImmediate(resolve));
const A: EventsQuery = { repositoryId: "A", state: EventState.PENDING };
const B: EventsQuery = { repositoryId: "B", state: EventState.COMPLETED };
const page = (item: string, nextCursor = "") => ({ items: [item], nextCursor });
const names = (items: unknown[]) => items.map((i) => String(i));

test("filter change: B stays visible when A resolves later in every order", async () => {
  for (const order of ["receivers", "events"] as const) {
    const h = harness();
    const a = h.loader.reload(A);
    // A's receivers are delayed (or answered, with only its events delayed).
    if (order === "events") {
      h.calls[0].resolve(page("A-receiver"));
      await tick();
    }
    const b = h.loader.reload(B);
    const bFirst = h.calls.findIndex(
      (c, i) => i > (order === "events" ? 1 : 0) && c.kind === "receivers",
    );
    h.calls[bFirst].resolve(page("B-receiver"));
    await tick();
    h.calls[bFirst + 1].resolve(page("B-event", "B-next"));
    await b;
    // Now the obsolete A requests finish.
    for (const call of h.calls.slice(0, bFirst)) {
      if (call.kind === "receivers" && order === "receivers")
        call.resolve(page("A-receiver"));
    }
    await tick();
    h.calls
      .filter((c, i) => c.kind === "events" && i < bFirst)
      .forEach((c) => c.resolve(page("A-event", "A-next")));
    await a;
    const v = h.view();
    assert.deepEqual(names(v.receivers), ["B-receiver"]);
    assert.deepEqual(names(v.events), ["B-event"]);
    assert.equal(v.cursor, "B-next");
    assert.equal(v.cursorKey, eventsQueryKey(B));
    assert.equal(v.error, "");
    assert.equal(v.loading, false);
  }
});

test("an obsolete load abandons its receiver page chain and cannot set an error", async () => {
  const h = harness();
  const a = h.loader.reload(A);
  h.calls[0].resolve(page("A-receiver-1", "more"));
  await tick();
  const b = h.loader.reload(B);
  // A's second receiver page was requested before B began; it must not chain further.
  h.calls[1].resolve(page("A-receiver-2", "even-more"));
  await tick();
  assert.equal(
    h.calls.filter(
      (c) => c.kind === "receivers" && c.input.page.cursor === "even-more",
    ).length,
    0,
  );
  h.calls[2].resolve(page("B-receiver"));
  await tick();
  h.calls[3].reject(new Error("B failed"));
  await b;
  await a;
  // Only B's own failure is visible; A's late chain published nothing.
  assert.equal(h.view().error, "B failed");
  assert.deepEqual(names(h.view().receivers), ["B-receiver"]);
});

test("obsolete failure cannot replace the current query status", async () => {
  const h = harness();
  const a = h.loader.reload(A);
  const b = h.loader.reload(B);
  h.calls[0].reject(new Error("A failed"));
  await a;
  assert.equal(h.view().error, "");
  assert.equal(h.view().loading, true);
  h.calls[1].resolve(page("B-receiver"));
  await tick();
  h.calls[2].resolve(page("B-event"));
  await b;
  assert.equal(h.view().error, "");
  assert.deepEqual(names(h.view().events), ["B-event"]);
});

test("a late A continuation cannot replace or extend B", async () => {
  const h = harness();
  const first = h.loader.reload(A);
  h.calls[0].resolve(page("A-receiver"));
  await tick();
  h.calls[1].resolve(page("A-page-1", "A-next"));
  await first;
  const more = h.loader.loadMore(A);
  assert.equal(h.calls[2].input.page.cursor, "A-next");
  const b = h.loader.reload(B);
  h.calls[3].resolve(page("B-receiver"));
  await tick();
  h.calls[4].resolve(page("B-event", "B-next"));
  await b;
  h.calls[2].resolve(page("A-page-2", "A-final"));
  await more;
  const v = h.view();
  assert.deepEqual(names(v.events), ["B-event"]);
  assert.equal(v.cursor, "B-next");
  assert.equal(v.loadingMore, false);
});

test("a new query never sends the previous cursor, pending or failed", async () => {
  const h = harness();
  const first = h.loader.reload(A);
  h.calls[0].resolve(page("A-receiver"));
  await tick();
  h.calls[1].resolve(page("A-page-1", "A-next"));
  await first;
  const b = h.loader.reload(B);
  assert.equal(h.view().cursor, "");
  // Pagination from the stale render for A, and for B, is refused while B is pending.
  await h.loader.loadMore(A);
  await h.loader.loadMore(B);
  assert.equal(h.calls.length, 3);
  h.calls[2].resolve(page("B-receiver"));
  await tick();
  h.calls[3].reject(new Error("B failed"));
  await b;
  assert.equal(h.view().error, "B failed");
  await h.loader.loadMore(B);
  await h.loader.loadMore(A);
  assert.equal(h.calls.length, 4);
  assert.equal(
    h.calls.some(
      (c) => c.input.page.cursor === "A-next" && c.input.repositoryId === "B",
    ),
    false,
  );
  // Explicit retry succeeds and its own continuation works.
  const retry = h.loader.refresh();
  assert.equal(h.view().error, "");
  h.calls[4].resolve(page("B-receiver"));
  await tick();
  h.calls[5].resolve(page("B-page-1", "B-next"));
  await retry;
  const more = h.loader.loadMore(B);
  assert.equal(h.calls[6].input.page.cursor, "B-next");
  assert.equal(h.calls[6].input.repositoryId, "B");
  h.calls[6].resolve(page("B-page-2"));
  await more;
  assert.deepEqual(names(h.view().events), ["B-page-1", "B-page-2"]);
  assert.equal(h.view().cursor, "");
});

test("refresh overlapping a filter change publishes only the owning load", async () => {
  const h = harness();
  const a = h.loader.reload(A);
  const refresh = h.loader.refresh(); // same latest query A
  const b = h.loader.reload(B);
  // Resolve in reverse order of start: B, refresh, A.
  h.calls[2].resolve(page("B-receiver"));
  await tick();
  h.calls[3].resolve(page("B-event"));
  await b;
  h.calls[1].reject(new Error("refresh failed"));
  await refresh;
  h.calls[0].resolve(page("A-receiver"));
  await a;
  const v = h.view();
  assert.deepEqual(names(v.receivers), ["B-receiver"]);
  assert.deepEqual(names(v.events), ["B-event"]);
  assert.equal(v.error, "");
});

test("refresh overlapping a continuation replaces the list without duplicates", async () => {
  const h = harness();
  const first = h.loader.reload(A);
  h.calls[0].resolve(page("receiver"));
  await tick();
  h.calls[1].resolve(page("page-1", "next"));
  await first;
  const more = h.loader.loadMore(A);
  // A second continuation while one is in flight is rejected.
  await h.loader.loadMore(A);
  assert.equal(h.calls.length, 3);
  const refresh = h.loader.refresh();
  h.calls[3].resolve(page("receiver"));
  await tick();
  h.calls[4].resolve(page("page-1", "next"));
  await refresh;
  h.calls[2].resolve(page("page-2", "stale-next"));
  await more;
  assert.deepEqual(names(h.view().events), ["page-1"]);
  assert.equal(h.view().cursor, "next");
  // A continuation failing after being superseded by a failing refresh cannot show.
  const more2 = h.loader.loadMore(A);
  h.calls[5].resolve(page("page-2"));
  await more2;
  assert.deepEqual(names(h.view().events), ["page-1", "page-2"]);
  assert.equal(h.view().cursor, "");
});

test("single query retains every receiver page and appends event pages once", async () => {
  const h = harness();
  const first = h.loader.reload(A);
  h.calls[0].resolve(page("receiver-1", "r2"));
  await tick();
  assert.equal(h.calls[1].input.page.cursor, "r2");
  h.calls[1].resolve(page("receiver-2"));
  await tick();
  h.calls[2].resolve(page("event-1", "e2"));
  await first;
  assert.deepEqual(names(h.view().receivers), ["receiver-1", "receiver-2"]);
  const more = h.loader.loadMore(A);
  assert.equal(h.calls[3].kind, "events");
  assert.equal(h.calls[3].input.page.cursor, "e2");
  h.calls[3].resolve(page("event-2"));
  await more;
  assert.deepEqual(names(h.view().events), ["event-1", "event-2"]);
  assert.deepEqual(names(h.view().receivers), ["receiver-1", "receiver-2"]);
  assert.equal(h.view().cursor, "");
  await h.loader.loadMore(A);
  assert.equal(h.calls.length, 4);
});

test("current-query continuation failure keeps the cursor for retry", async () => {
  const h = harness();
  const first = h.loader.reload(A);
  h.calls[0].resolve(page("receiver"));
  await tick();
  h.calls[1].resolve(page("event-1", "e2"));
  await first;
  const more = h.loader.loadMore(A);
  h.calls[2].reject(new Error("page failed"));
  await more;
  assert.equal(h.view().error, "page failed");
  assert.deepEqual(names(h.view().events), ["event-1"]);
  assert.equal(h.view().cursor, "e2");
  const retry = h.loader.loadMore(A);
  assert.equal(h.view().error, "");
  h.calls[3].resolve(page("event-2"));
  await retry;
  assert.deepEqual(names(h.view().events), ["event-1", "event-2"]);
});

test("a disposed loader never publishes", async () => {
  const h = harness();
  const first = h.loader.reload(A);
  h.calls[0].resolve(page("receiver"));
  await tick();
  const before = h.view();
  h.loader.dispose();
  h.calls[1].resolve(page("event", "next"));
  await first;
  assert.equal(h.view(), before);
  await h.loader.reload(B);
  await h.loader.refresh();
  assert.equal(h.calls.length, 2);
  assert.equal(h.view(), before);
});

// An invalidation reloads the unchanged active query through reload(), the same
// call the Events effect makes when App's revision changes.
test("invalidation reload keeps rows visible, then replaces the first page and cursor", async () => {
  const h = harness();
  const first = h.loader.reload(A);
  h.calls[0].resolve(page("old-receiver"));
  await tick();
  h.calls[1].resolve(page("old-event", "old-next"));
  await first;
  const more = h.loader.loadMore(A);
  h.calls[2].resolve({ items: ["old-event-2"], nextCursor: "old-next-2" });
  await more;
  assert.deepEqual(names(h.view().events), ["old-event", "old-event-2"]);

  const invalidated = h.loader.reload(A);
  // Same query: rows stay on screen while the first page is reread, but the
  // old continuation is gone and the reread starts from the first page.
  assert.deepEqual(names(h.view().events), ["old-event", "old-event-2"]);
  assert.equal(h.view().cursor, "");
  assert.equal(h.view().loading, true);
  assert.equal(h.calls[3].kind, "receivers");
  assert.equal(h.calls[3].input.repositoryId, "A");
  assert.equal(h.calls[3].input.page.cursor, "");
  h.calls[3].resolve(page("new-receiver"));
  await tick();
  assert.equal(h.calls[4].kind, "events");
  assert.equal(h.calls[4].input.page.cursor, "");
  assert.deepEqual(h.calls[4].input.states, [EventState.PENDING]);
  h.calls[4].resolve(page("new-event", "new-next"));
  await invalidated;
  const v = h.view();
  assert.deepEqual(names(v.receivers), ["new-receiver"]);
  assert.deepEqual(names(v.events), ["new-event"]);
  assert.equal(v.cursor, "new-next");
  assert.equal(v.cursorKey, eventsQueryKey(A));
  assert.equal(v.loading, false);

  // The next continuation uses the refreshed cursor and appends once.
  const next = h.loader.loadMore(A);
  assert.equal(h.calls[5].input.page.cursor, "new-next");
  h.calls[5].resolve(page("new-event-2"));
  await next;
  assert.deepEqual(names(h.view().events), ["new-event", "new-event-2"]);
  assert.equal(h.view().cursor, "");
  assert.equal(h.calls.length, 6);
});

test("invalidation supersedes an old load and a held old response or error is ignored", async () => {
  for (const settle of ["resolve", "reject"] as const) {
    const h = harness();
    const old = h.loader.reload(A);
    const invalidated = h.loader.reload(A);
    h.calls[1].resolve(page("new-receiver"));
    await tick();
    h.calls[2].resolve(page("new-event"));
    await invalidated;
    // Release the old load last.
    if (settle === "resolve") h.calls[0].resolve(page("old-receiver"));
    else h.calls[0].reject(new Error("old failed"));
    await old;
    const v = h.view();
    assert.deepEqual(names(v.receivers), ["new-receiver"]);
    assert.deepEqual(names(v.events), ["new-event"]);
    assert.equal(v.error, "");
    assert.equal(v.loading, false);
    assert.equal(h.calls.length, 3);
  }
});

test("invalidation supersedes an in-flight continuation without duplicate rows", async () => {
  const h = harness();
  const first = h.loader.reload(A);
  h.calls[0].resolve(page("receiver"));
  await tick();
  h.calls[1].resolve(page("event-1", "next"));
  await first;
  const more = h.loader.loadMore(A);
  const invalidated = h.loader.reload(A);
  h.calls[3].resolve(page("receiver"));
  await tick();
  h.calls[4].resolve(page("event-1", "fresh-next"));
  await invalidated;
  // The held continuation fails last and also must not show an error.
  h.calls[2].reject(new Error("continuation failed"));
  await more;
  assert.deepEqual(names(h.view().events), ["event-1"]);
  assert.equal(h.view().cursor, "fresh-next");
  assert.equal(h.view().error, "");
  assert.equal(h.view().loadingMore, false);
});

test("invalidation after a filter change rereads only the latest filter", async () => {
  const h = harness();
  const a = h.loader.reload(A);
  const b = h.loader.reload(B);
  const invalidated = h.loader.reload(B);
  assert.deepEqual(
    h.calls.map((c) => c.input.repositoryId),
    ["A", "B", "B"],
  );
  h.calls[2].resolve(page("B-receiver-new"));
  await tick();
  h.calls[3].resolve(page("B-event-new"));
  await invalidated;
  h.calls[1].resolve(page("B-receiver-old"));
  await tick();
  h.calls[0].resolve(page("A-receiver"));
  await Promise.all([a, b]);
  assert.deepEqual(names(h.view().receivers), ["B-receiver-new"]);
  assert.deepEqual(names(h.view().events), ["B-event-new"]);
});

test("a failed invalidation reload is visible and an explicit retry recovers it", async () => {
  const h = harness();
  const first = h.loader.reload(A);
  h.calls[0].resolve(page("receiver"));
  await tick();
  h.calls[1].resolve(page("event-1"));
  await first;
  const invalidated = h.loader.reload(A);
  h.calls[2].reject(new Error("notification load failed"));
  await invalidated;
  assert.equal(h.view().error, "notification load failed");
  assert.equal(h.view().loading, false);
  // The previous rows remain; manual refresh retries and clears the error.
  assert.deepEqual(names(h.view().events), ["event-1"]);
  const retry = h.loader.refresh();
  assert.equal(h.view().error, "");
  h.calls[3].resolve(page("receiver-2"));
  await tick();
  h.calls[4].resolve(page("event-2"));
  await retry;
  assert.equal(h.view().error, "");
  assert.deepEqual(names(h.view().events), ["event-2"]);
});

test("an invalidation after dispose or adapter replacement publishes nothing", async () => {
  const h = harness();
  const first = h.loader.reload(A);
  h.calls[0].resolve(page("receiver"));
  await tick();
  h.calls[1].resolve(page("event"));
  await first;
  const before = h.view();
  h.loader.dispose();
  await h.loader.reload(A);
  assert.equal(h.calls.length, 2);
  assert.equal(h.view(), before);
});

// Deferred GetEvent adapter: every read waits until the test settles it. Any
// method other than GetEvent fails the test, so reads cannot mutate events.
interface DetailCall {
  id: string;
  resolve(value: { event?: { id: string }; processing: string[] }): void;
  reject(error: Error): void;
}

function detailHarness() {
  const calls: DetailCall[] = [];
  const adapter = {
    call(method: unknown, input: { id: string }) {
      assert.equal(method, WebhookService.method.getEvent);
      return new Promise((resolve, reject) =>
        calls.push({
          id: input.id,
          resolve: resolve as DetailCall["resolve"],
          reject,
        }),
      );
    },
  } as unknown as LaneAdapter;
  let view: EventDetailView = emptyEventDetailView;
  const published: EventDetailView[] = [];
  const reader = new EventDetailReader(adapter, (v) => {
    view = v;
    published.push(v);
  });
  return { calls, reader, view: () => view, published };
}

const reply = (id: string, payload: string) => ({
  event: { id, payload },
  processing: [`${id}-history`],
});
const shown = (view: EventDetailView) => ({
  id: view.selected?.event.id,
  payload: (view.selected?.event as { payload?: string } | undefined)?.payload,
  processing: view.selected?.processing.map(String),
});

test("detail: a late A success cannot replace the accepted B", async () => {
  const h = detailHarness();
  const a = h.reader.inspect("A");
  const b = h.reader.inspect("B");
  h.calls[1].resolve(reply("B", "payload-B"));
  await b;
  assert.deepEqual(shown(h.view()), {
    id: "B",
    payload: "payload-B",
    processing: ["B-history"],
  });
  const before = h.published.length;
  h.calls[0].resolve(reply("A", "payload-A"));
  await a;
  assert.equal(h.published.length, before);
  assert.deepEqual(shown(h.view()), {
    id: "B",
    payload: "payload-B",
    processing: ["B-history"],
  });
  assert.equal(h.view().error, "");
  assert.equal(h.view().loading, false);
});

test("detail: a late A failure publishes no obsolete status", async () => {
  const h = detailHarness();
  const a = h.reader.inspect("A");
  const b = h.reader.inspect("B");
  h.calls[1].resolve(reply("B", "payload-B"));
  await b;
  const before = h.published.length;
  h.calls[0].reject(new Error("obsolete A failure"));
  await a;
  assert.equal(h.published.length, before);
  assert.equal(h.view().error, "");
  assert.equal(h.view().selected?.event.id, "B");
});

test("detail: an obsolete read settling while B is pending keeps B loading", async () => {
  const h = detailHarness();
  const a = h.reader.inspect("A");
  const b = h.reader.inspect("B");
  h.calls[0].resolve(reply("A", "payload-A"));
  await a;
  assert.equal(h.view().selected, undefined);
  assert.equal(h.view().loading, true);
  h.calls[1].resolve(reply("B", "payload-B"));
  await b;
  assert.equal(h.view().selected?.event.id, "B");
  assert.equal(h.view().loading, false);
});

test("detail: closing ends ownership so older reads cannot reopen or fail", async () => {
  for (const settle of ["resolve", "reject"] as const) {
    const h = detailHarness();
    const a = h.reader.inspect("A");
    const b = h.reader.inspect("B");
    h.calls[1].resolve(reply("B", "payload-B"));
    await b;
    h.reader.close();
    assert.equal(h.view().selected, undefined);
    const before = h.published.length;
    if (settle === "resolve") h.calls[0].resolve(reply("A", "payload-A"));
    else h.calls[0].reject(new Error("obsolete A failure"));
    await a;
    assert.equal(h.published.length, before);
    assert.equal(h.view().selected, undefined);
    assert.equal(h.view().error, "");
  }
});

test("detail: a pending current read closed before it settles stays closed", async () => {
  const h = detailHarness();
  const a = h.reader.inspect("A");
  h.reader.close();
  assert.equal(h.view().loading, false);
  h.calls[0].resolve(reply("A", "payload-A"));
  await a;
  assert.equal(h.view().selected, undefined);
});

test("detail: A then B then A keeps the newest A content", async () => {
  const h = detailHarness();
  const first = h.reader.inspect("A");
  const second = h.reader.inspect("B");
  const third = h.reader.inspect("A");
  assert.deepEqual(
    h.calls.map((c) => c.id),
    ["A", "B", "A"],
  );
  h.calls[2].resolve(reply("A", "new-A"));
  await third;
  h.calls[0].resolve(reply("A", "old-A"));
  await first;
  h.calls[1].resolve(reply("B", "payload-B"));
  await second;
  assert.deepEqual(shown(h.view()), {
    id: "A",
    payload: "new-A",
    processing: ["A-history"],
  });
});

test("detail: disposal invalidates pending reads", async () => {
  const h = detailHarness();
  const a = h.reader.inspect("A");
  h.reader.dispose();
  const before = h.published.length;
  h.calls[0].resolve(reply("A", "payload-A"));
  await a;
  const failed = h.reader.inspect("B");
  await failed;
  assert.equal(h.calls.length, 1);
  assert.equal(h.published.length, before);
  assert.equal(h.view().selected, undefined);
});

test("detail: closing for Open linked task invalidates an older A, even after a newer A", async () => {
  const h = detailHarness();
  const old = h.reader.inspect("A");
  const newest = h.reader.inspect("A");
  h.calls[1].resolve(reply("A", "new-A"));
  await newest;
  // Open linked task closes the detail through the same close path.
  h.reader.close();
  h.calls[0].resolve(reply("A", "old-A"));
  await old;
  assert.equal(h.view().selected, undefined);
});

test("detail: a single current read, a later close and flushed callbacks stay closed", async () => {
  const h = detailHarness();
  const a = h.reader.inspect("A");
  assert.equal(h.view().loading, true);
  h.calls[0].resolve(reply("A", "payload-A"));
  await a;
  assert.equal(h.view().selected?.event.id, "A");
  assert.equal(h.view().loading, false);
  h.reader.close();
  await tick();
  assert.equal(h.view().selected, undefined);
});

test("detail: reads in request order end on the final B", async () => {
  const h = detailHarness();
  const a = h.reader.inspect("A");
  h.calls[0].resolve(reply("A", "payload-A"));
  await a;
  const b = h.reader.inspect("B");
  h.calls[1].resolve(reply("B", "payload-B"));
  await b;
  assert.equal(h.view().selected?.event.id, "B");
});

test("detail: a missing event while closed opens nothing and a missing event while open keeps the detail", async () => {
  const h = detailHarness();
  const closed = h.reader.inspect("A");
  h.calls[0].resolve({ processing: [] });
  await closed;
  assert.equal(h.view().selected, undefined);
  assert.equal(h.view().error, "");
  assert.equal(h.view().loading, false);
  const b = h.reader.inspect("B");
  h.calls[1].resolve(reply("B", "payload-B"));
  await b;
  const missing = h.reader.inspect("C");
  h.calls[2].resolve({ processing: [] });
  await missing;
  assert.equal(h.view().selected?.event.id, "B");
});

test("detail: a current failure is visible and an explicit retry recovers", async () => {
  const h = detailHarness();
  const failed = h.reader.inspect("A");
  h.calls[0].reject(new Error("read failed"));
  await failed;
  assert.equal(h.view().error, "read failed");
  assert.equal(h.view().loading, false);
  assert.equal(h.view().selected, undefined);
  const retry = h.reader.inspect("A");
  assert.equal(h.view().error, "");
  assert.equal(h.view().loading, true);
  h.calls[1].resolve(reply("A", "payload-A"));
  await retry;
  assert.equal(h.view().error, "");
  assert.equal(h.view().selected?.event.id, "A");
});

test("detail: update keeps the accepted history and is ignored while closed", async () => {
  const h = detailHarness();
  h.reader.update({ id: "B" } as never);
  assert.equal(h.view().selected, undefined);
  const b = h.reader.inspect("B");
  h.calls[0].resolve(reply("B", "payload-B"));
  await b;
  h.reader.update({ id: "B", payload: "reopened" } as never);
  assert.deepEqual(shown(h.view()), {
    id: "B",
    payload: "reopened",
    processing: ["B-history"],
  });
  // An obsolete read cannot undo the accepted update.
  const a = h.reader.inspect("A");
  const c = h.reader.inspect("B");
  h.calls[2].resolve(reply("B", "latest"));
  await c;
  h.calls[1].resolve(reply("A", "payload-A"));
  await a;
  assert.equal(h.view().selected?.event.id, "B");
  assert.equal(
    (h.view().selected?.event as { payload?: string }).payload,
    "latest",
  );
});

test("detail: a delayed mutation response for A cannot replace the open B", async () => {
  const h = detailHarness();
  const a = h.reader.inspect("A");
  h.calls[0].resolve(reply("A", "payload-A"));
  await a;
  // The user starts reopening A, closes its details and inspects B.
  h.reader.close();
  const b = h.reader.inspect("B");
  h.calls[1].resolve(reply("B", "payload-B"));
  await b;
  const before = h.published.length;
  h.reader.update({ id: "A", payload: "reopened-A" } as never);
  assert.equal(h.published.length, before);
  assert.deepEqual(shown(h.view()), {
    id: "B",
    payload: "payload-B",
    processing: ["B-history"],
  });
  // A response for the accepted event still applies and keeps its history.
  h.reader.update({ id: "B", payload: "reopened-B" } as never);
  assert.deepEqual(shown(h.view()), {
    id: "B",
    payload: "reopened-B",
    processing: ["B-history"],
  });
});
