// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { desktopModules } from "../src/desktop.ts";
import { invokeNative } from "../native/desktop/bridge.mjs";

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
    modules.store.open('{"formatVersion":1,"spaceId":""}', resolve),
  );
  assert.equal(result, expected);
  assert.deepEqual(received, [{ formatVersion: 1, spaceId: "" }]);
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
