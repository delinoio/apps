// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import {
  NativeAdapter,
  decodeReply,
  newId,
  type LaneStoreModule,
  type EventSource,
} from "../src/adapter.ts";
import { LaneService } from "@delino/protos/lane/v1/lane_pb";
function harness() {
  const calls: { method: string; request: Record<string, unknown> }[] = [];
  const listeners = new Map<
    string,
    (e: { spaceId: string; sequence: string }) => void
  >();
  const module: LaneStoreModule = {
    open(_req, cb) {
      cb(
        JSON.stringify({
          ok: true,
          response: {
            spaceId: "space",
            deviceId: "device",
            formatVersion: 1,
            schemaVersion: 1,
            sequence: "0",
          },
        }),
      );
    },
    invoke(method, json, cb) {
      calls.push({ method, request: JSON.parse(json) });
      cb(JSON.stringify({ ok: true, response: { items: [], nextCursor: "" } }));
    },
    close() {},
  };
  const events: EventSource = {
    addListener(name, cb) {
      listeners.set(name, cb);
    },
    removeListener(name) {
      listeners.delete(name);
    },
  };
  return {
    adapter: new NativeAdapter(module, events),
    module,
    calls,
    listeners,
  };
}
test("native calls carry exact pinned wrapper and canonical protobuf enum names", async () => {
  const h = harness();
  await h.adapter.open();
  await h.adapter.call(LaneService.method.listTasks, {
    progress: [1],
    proposal: [2],
    search: "literal.*",
  });
  assert.deepEqual(h.calls[0], {
    method: "ListTasks",
    request: {
      spaceId: "space",
      request: {
        progress: ["PROGRESS_TODO"],
        proposal: ["PROPOSAL_PENDING"],
        search: "literal.*",
      },
    },
  });
});
test("native errors preserve reason and never report successful save", () => {
  assert.throws(
    () =>
      decodeReply({
        ok: false,
        error: {
          reason: "STORAGE_FULL",
          message: "Storage full",
          retryable: true,
        },
      }),
    /Storage full/,
  );
});
test("sequence events invalidate only selected space, deduplicate, and clean up", async () => {
  const h = harness();
  await h.adapter.open();
  let n = 0;
  const stop = h.adapter.subscribe(() => n++);
  const changed = h.listeners.get("lane.storeChanged")!;
  changed({ spaceId: "other", sequence: "1" });
  changed({ spaceId: "space", sequence: "1" });
  changed({ spaceId: "space", sequence: "1" });
  changed({ spaceId: "space", sequence: "2" });
  assert.equal(n, 2);
  stop();
  assert.equal(h.listeners.size, 0);
});
test("late callback cannot update a closed account space", async () => {
  const h = harness();
  await h.adapter.open();
  let finish: ((value: any) => void) | undefined;
  h.module.invoke = (_m, _r, cb) => {
    finish = cb;
  };
  const pending = h.adapter.call(LaneService.method.listTasks, {});
  h.adapter.close();
  finish!({ ok: true, response: { items: [] } });
  await assert.rejects(pending, /data space changed/);
});
test("unsupported schema refuses opening", async () => {
  const h = harness();
  h.module.open = (_r, cb) =>
    cb({
      ok: true,
      response: {
        spaceId: "s",
        deviceId: "d",
        schemaVersion: 2,
        formatVersion: 1,
        sequence: "0",
      },
    });
  await assert.rejects(h.adapter.open(), /Update Lane/);
});
test("request IDs are UUID v7", () => {
  for (let i = 0; i < 100; i++)
    assert.match(
      newId(),
      /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
});
