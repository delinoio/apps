// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { reconnectTarget, confirmReconnect } from "../src/reconnect.ts";
import type { LaneAdapter, LocalStatus } from "../src/adapter.ts";
const status: LocalStatus = {
  spaceId: "space",
  accountId: "account",
  deviceId: "device",
  syncEnabled: true,
  connected: false,
  pendingItemCount: "0",
  reconnectRequired: true,
};
function fixture(current: LocalStatus) {
  const calls: unknown[] = [];
  const adapter = {
    local: async (method: string, input: unknown) => {
      calls.push([method, input]);
      return method === "GetLocalStatus" ? current : {};
    },
  } as LaneAdapter;
  return { adapter, calls };
}
test("opening or canceling confirmation does not start approval", () => {
  const { calls } = fixture(status);
  assert.deepEqual(reconnectTarget(status), {
    spaceId: "space",
    accountId: "account",
  });
  assert.deepEqual(calls, []);
});
test("explicit confirmation rechecks pinned target before new targeted approval", async () => {
  const { adapter, calls } = fixture(status);
  await confirmReconnect(adapter, reconnectTarget(status));
  assert.deepEqual(calls, [
    ["GetLocalStatus", {}],
    ["ConnectSync", { accountId: "account" }],
  ]);
});
test("empty, mismatched, switched or no-longer-stale target never starts approval", async () => {
  for (const patch of [
    { accountId: "" },
    { accountId: "another" },
    { spaceId: "another" },
    { reconnectRequired: false },
  ]) {
    const { adapter, calls } = fixture({ ...status, ...patch });
    await assert.rejects(confirmReconnect(adapter, reconnectTarget(status)));
    assert.equal(calls.length, 1);
  }
});
test("ordinary first connection status is not reconnect consent; later stale refresh does not approve", async () => {
  assert.throws(() => reconnectTarget({ ...status, reconnectRequired: false }));
  const { adapter, calls } = fixture(status);
  await adapter.local("GetLocalStatus", {});
  assert.deepEqual(calls, [["GetLocalStatus", {}]]);
});
