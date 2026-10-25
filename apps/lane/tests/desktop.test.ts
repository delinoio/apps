// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { desktopModules } from "../src/desktop.ts";
import { invokeNative, WindowSession } from "../native/desktop/bridge.mjs";

test("Lynxtron preserves native requests and response envelopes", async () => {
  const received: unknown[] = [];
  const expected = JSON.stringify({
    ok: true,
    response: { spaceId: "test-space" },
    sequence: "42",
  });
  const native = {
    store: {
      open: (request: string, callback: (raw: string) => void) => {
        received.push(JSON.parse(request));
        callback(expected);
      },
    },
    host: {},
  };
  const modules = desktopModules({
    call: (channel, request, callback) => {
      assert.equal(channel, "lane.native");
      void invokeNative(native, request).then(callback);
    },
  });
  const result = await new Promise((resolve) =>
    modules.store.open('{"formatVersion":2,"spaceId":""}', resolve),
  );
  assert.equal(result, expected);
  assert.deepEqual(received, [{ formatVersion: 2, spaceId: "" }]);
});

test("Lynxtron rejects unlisted capabilities before calling native code", async () => {
  const native = new Proxy(
    {},
    {
      get: () => {
        throw new Error("Native dispatch must not run");
      },
    },
  );
  for (const request of [
    { module: "LaneStore", method: "__proto__", args: [] },
    { module: "constructor", method: "open", args: [""] },
    { module: "LaneStore", method: "open", args: [42] },
    { module: "LaneStore", method: "invoke", args: ["GetTask"] },
    { module: "LaneHost", method: "control", args: ["stop", ""] },
  ]) {
    const reply = JSON.parse(await invokeNative(native, request));
    assert.equal(reply.ok, false);
    assert.equal(reply.error.reason, "ERROR_REASON_INVALID_INPUT");
  }
});

test("Lynxtron transport failure returns a retryable safe error", async () => {
  const modules = desktopModules({
    call: () => {
      throw new Error("private native details");
    },
  });
  const raw = await new Promise<string>((resolve) =>
    modules.host.openURL("https://example.com", resolve),
  );
  assert.equal(
    JSON.parse(raw).error.reason,
    "ERROR_REASON_DEPENDENCY_UNAVAILABLE",
  );
  assert.equal(JSON.parse(raw).error.retryable, true);
  assert.equal(raw.includes("private native details"), false);
});

test("a delayed storage open cannot ready a replacement Lynxtron window", async () => {
  const callbacks: ((raw: string) => void)[] = [];
  const native = {
    store: {
      open: (_request: string, callback: (raw: string) => void) =>
        callbacks.push(callback),
    },
  };
  const request = {
    module: "LaneStore",
    method: "open",
    args: ['{"formatVersion":2,"spaceId":""}'],
  };
  const previous = new WindowSession();
  const oldOpen = previous.invoke(native, request);
  previous.retire();
  const replacement = new WindowSession();
  const newOpen = replacement.invoke(native, request);
  callbacks[0]('{"ok":true,"response":{}}');
  await oldOpen;
  assert.equal(previous.ready, false);
  assert.equal(replacement.ready, false);
  callbacks[1]('{"ok":true,"response":{}}');
  await newOpen;
  assert.equal(replacement.ready, true);
  replacement.retire();
  assert.equal(replacement.ready, false);
});

test("retired Lynxtron windows cannot dispatch new native requests", async () => {
  const session = new WindowSession();
  session.retire();
  const native = new Proxy(
    {},
    {
      get: () => {
        throw new Error("Retired renderer must not dispatch");
      },
    },
  );
  const result = await session.invoke(native, {
    module: "LaneStore",
    method: "open",
    args: [""],
  });
  assert.equal(JSON.parse(result).ok, false);
  assert.equal(session.ready, false);
});
