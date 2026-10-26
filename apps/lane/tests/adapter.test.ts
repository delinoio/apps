// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import {
  NativeAdapter,
  decodeReply,
  LaneError,
  newId,
  type LaneStoreModule,
  type EventSource,
} from "../src/adapter.ts";
import { LaneService } from "@repo/protos/lane/v1/lane_pb";
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
            spaceId: JSON.parse(_req).spaceId || "space",
            deviceId: "device",
            formatVersion: 3,
            schemaVersion: 3,
            sequence: "0",
          },
        }),
      );
    },
    invoke(method, json, cb) {
      calls.push({ method, request: JSON.parse(json) });
      cb(JSON.stringify({ ok: true, response: { items: [], nextCursor: "" } }));
    },
    close(_space, callback) {
      callback({ ok: true, response: { completed: true } });
    },
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
        schemaVersion: 99,
        formatVersion: 3,
        sequence: "0",
      },
    });
  await assert.rejects(h.adapter.open(), /unsupported format/);
});
test("request IDs are UUID v7", () => {
  for (let i = 0; i < 100; i++)
    assert.match(
      newId(),
      /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
});
test("explicit route opens exactly the named space with no fallback on error", async () => {
  const h = harness();
  await h.adapter.open();
  const requests: unknown[] = [];
  let closes = 0;
  h.module.close = (_space, callback) => {
    closes++;
    callback({ ok: true, response: { completed: true } });
  };
  h.module.open = (raw, cb) => {
    requests.push(JSON.parse(raw));
    cb({ ok: false, error: { reason: "NOT_FOUND", message: "Space removed" } });
  };
  await assert.rejects(h.adapter.open("named-space"), /Space removed/);
  assert.deepEqual(requests, [{ formatVersion: 3, spaceId: "named-space" }]);
  assert.equal(closes, 1);
  await assert.rejects(
    h.adapter.call(LaneService.method.listTasks, {}),
    /not open/,
  );
});
test("late read from old space is rejected after a new space opens", async () => {
  const h = harness();
  await h.adapter.open();
  let finish: ((raw: any) => void) | undefined;
  h.module.invoke = (_m, _r, cb) => {
    finish = cb;
  };
  const pending = h.adapter.call(LaneService.method.listTasks, {});
  h.module.open = (_r, cb) =>
    cb({
      ok: true,
      response: {
        spaceId: "new-space",
        deviceId: "device",
        schemaVersion: 3,
        formatVersion: 3,
        sequence: "0",
      },
    });
  await h.adapter.open("new-space");
  finish!({ ok: true, response: { items: [] } });
  await assert.rejects(pending, /data space changed/);
});

test("sequence invalidation compares decimal values beyond safe integers without rounding", async () => {
  const h = harness();
  await h.adapter.open();
  let count = 0;
  h.adapter.subscribe(() => count++);
  const event = h.listeners.get("lane.storeChanged")!;
  for (const sequence of [
    "9007199254740992",
    "9007199254740993",
    "9007199254740992",
    "9007199254740993",
    "18446744073709551615",
  ])
    event({ spaceId: "space", sequence });
  assert.equal(count, 3);
});

test("native deletion is explicit, challenge-bound, and sends no proof to JavaScript", async () => {
  const { AccountService } = await import("@repo/protos/lane/v1/lane_pb");
  const h = harness();
  await h.adapter.open();
  h.module.invoke = (method, json, cb) => {
    h.calls.push({ method, request: JSON.parse(json) });
    cb(
      JSON.stringify({
        ok: true,
        response:
          method === "BeginDataDeletion"
            ? {
                challengeId: "challenge",
                authorizationUrl: "https://auth.test",
              }
            : {},
      }),
    );
  };
  await h.adapter.call(AccountService.method.beginDataDeletion, {
    mode: 2,
    confirmedAccountId: "account",
    meta: { requestId: "begin" },
  });
  assert.equal(h.calls.length, 1);
  await assert.rejects(h.adapter.deleteReauthenticated("wrong"));
  await h.adapter.deleteReauthenticated("challenge");
  const request = h.calls[1].request;
  assert.equal(request.spaceId, "space");
  assert.deepEqual(Object.keys(request.request as object).sort(), [
    "challengeId",
    "confirmedAccountId",
    "meta",
    "mode",
  ]);
  assert.equal((request.request as any).challengeId, "challenge");
  await assert.rejects(h.adapter.deleteReauthenticated("challenge"));
});
test("cancel and space change invalidate native deletion challenge", async () => {
  const { AccountService } = await import("@repo/protos/lane/v1/lane_pb");
  for (const action of ["cancel", "switch"]) {
    const h = harness();
    await h.adapter.open();
    h.module.invoke = (_m, _r, cb) =>
      cb(JSON.stringify({ ok: true, response: { challengeId: "challenge" } }));
    await h.adapter.call(AccountService.method.beginDataDeletion, {
      mode: 1,
      confirmedAccountId: "account",
    });
    if (action === "cancel") h.adapter.cancelDeletion();
    else await h.adapter.open("another");
    await assert.rejects(h.adapter.deleteReauthenticated("challenge"));
  }
});

test("an earlier store format is refused with reset guidance and never migrated", async () => {
  const h = harness();
  const closed: string[] = [];
  h.module.open = (_r, cb) =>
    cb({
      ok: true,
      response: {
        spaceId: "old",
        deviceId: "d",
        schemaVersion: 2,
        formatVersion: 2,
        sequence: "0",
      },
    });
  h.module.close = (space, callback) => {
    closed.push(space);
    callback({ ok: true, response: { completed: true } });
  };
  await assert.rejects(
    h.adapter.open("old"),
    /never migrates or resets it automatically/,
  );
  assert.deepEqual(closed, ["old"]);
  assert.equal("migrate" in h.adapter, false);
  await assert.rejects(
    h.adapter.call(LaneService.method.listTasks, {}),
    /not open/,
  );
});
test("development reset is an explicit local operation that carries the confirmation flag", async () => {
  const h = harness();
  await h.adapter.open();
  const result = await h.adapter.local("ResetDevelopmentData", {
    confirmed: true,
  });
  assert.deepEqual(result, { items: [], nextCursor: "" });
  assert.deepEqual(h.calls.at(-1), {
    method: "ResetDevelopmentData",
    request: { spaceId: "space", request: { confirmed: true } },
  });
});
test("constraint violations keep their named code through the bridge error", () => {
  assert.throws(
    () =>
      decodeReply({
        ok: false,
        error: {
          reason: "CONSTRAINT_VIOLATION",
          message: "Parent loop",
          violation: "PARENT_CYCLE",
        },
      }),
    (error: unknown) =>
      error instanceof LaneError && error.detail.violation === "PARENT_CYCLE",
  );
});
test("an unsupported store can be reset without opening it, using the active space and the confirmation flag", async () => {
  const h = harness();
  h.module.open = (_r, cb) =>
    cb({
      ok: false,
      error: {
        reason: "ERROR_REASON_UNSUPPORTED_FORMAT",
        message: "Run the explicit development reset.",
      },
    });
  await assert.rejects(h.adapter.open(), /explicit development reset/);
  await h.adapter.resetUnopened!();
  assert.deepEqual(h.calls.at(-1), {
    method: "ResetDevelopmentData",
    request: { spaceId: "", request: { confirmed: true } },
  });
  // Rejections stay visible to the user.
  h.module.invoke = (_m, _r, cb) =>
    cb({
      ok: false,
      error: { reason: "STORAGE_BUSY", message: "Close other Lane windows." },
    });
  await assert.rejects(h.adapter.resetUnopened!(), /Close other Lane windows/);
});
test("blocked synchronization items are retried through the local operation", async () => {
  const h = harness();
  await h.adapter.open();
  await h.adapter.local("RetryBlockedItems", {});
  assert.deepEqual(h.calls.at(-1), {
    method: "RetryBlockedItems",
    request: { spaceId: "space", request: {} },
  });
});
