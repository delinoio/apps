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
    modules.store.open('{"formatVersion":3,"spaceId":""}', resolve),
  );
  assert.equal(result, expected);
  assert.deepEqual(received, [{ formatVersion: 3, spaceId: "" }]);
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
  const closed: string[] = [];
  const native = {
    store: {
      open: (_request: string, callback: (raw: string) => void) =>
        callbacks.push(callback),
      close: (spaceId: string, callback: (raw: string) => void) => {
        closed.push(spaceId);
        callback('{"ok":true,"response":{"completed":true}}');
      },
    },
  };
  const request = {
    module: "LaneStore",
    method: "open",
    args: ['{"formatVersion":3,"spaceId":""}'],
  };
  const previous = new WindowSession();
  const oldOpen = previous.invoke(native, request);
  previous.retire();
  const replacement = new WindowSession();
  const newOpen = replacement.invoke(native, request);
  await Promise.resolve();
  callbacks[0]('{"ok":true,"response":{"spaceId":"old-space"}}');
  await oldOpen;
  await previous.retire();
  assert.equal(previous.ready, false);
  assert.equal(replacement.ready, false);
  callbacks[1]('{"ok":true,"response":{"spaceId":"new-space"}}');
  await newOpen;
  assert.equal(replacement.ready, true);
  await replacement.retire();
  assert.equal(replacement.ready, false);
  assert.deepEqual(closed, ["old-space", "new-space"]);
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

const storeRequest = (method: "open" | "close", value: string) => ({
  module: "LaneStore",
  method,
  args: [value],
});
const opened = (spaceId: string) =>
  JSON.stringify({ ok: true, response: { spaceId } });
const closed = '{"ok":true,"response":{"completed":true}}';

test("retirement releases only its window's counted handles", async () => {
  const counts = new Map<string, number>();
  const closeCalls: string[] = [];
  const native = {
    store: {
      open: (request: string, callback: (raw: string) => void) => {
        const spaceId = JSON.parse(request).spaceId || "shared";
        counts.set(spaceId, (counts.get(spaceId) ?? 0) + 1);
        callback(opened(spaceId));
      },
      close: (spaceId: string, callback: (raw: string) => void) => {
        closeCalls.push(spaceId);
        counts.set(spaceId, (counts.get(spaceId) ?? 0) - 1);
        callback(closed);
      },
    },
  };
  const main = new WindowSession();
  for (const spaceId of ["shared", "shared", "other"])
    assert.equal(
      JSON.parse(
        await main.invoke(
          native,
          storeRequest("open", JSON.stringify({ spaceId })),
        ),
      ).ok,
      true,
    );
  for (let cycle = 0; cycle < 2; cycle++) {
    const composer = new WindowSession();
    await composer.invoke(
      native,
      storeRequest("open", JSON.stringify({ spaceId: "shared" })),
    );
    assert.equal(
      JSON.parse(await composer.invoke(native, storeRequest("close", "other")))
        .ok,
      false,
    );
    await composer.retire();
  }
  assert.equal(counts.get("shared"), 2);
  assert.equal(counts.get("other"), 1);
  assert.equal(main.ready, true);
  await main.invoke(native, storeRequest("close", "shared"));
  await main.retire();
  await main.retire();
  assert.deepEqual([...counts.values()], [0, 0]);
  assert.deepEqual(closeCalls, [
    "shared",
    "shared",
    "shared",
    "shared",
    "other",
  ]);
});

test("retirement drains opens completed before and after their callbacks", async () => {
  const callbacks: Array<(raw: string) => void> = [];
  let count = 0;
  let closeCalls = 0;
  const native = {
    store: {
      open: (_request: string, callback: (raw: string) => void) => {
        count++;
        callbacks.push(callback);
      },
      close: (_spaceId: string, callback: (raw: string) => void) => {
        count--;
        closeCalls++;
        callback(closed);
      },
    },
  };
  const first = new WindowSession();
  const beforeExecution = first.invoke(native, storeRequest("open", "{}"));
  const firstDrain = first.retire();
  await Promise.resolve();
  assert.equal(count, 1);
  callbacks.shift()?.(opened("shared"));
  await beforeExecution;
  await firstDrain;
  assert.equal(count, 0);

  const second = new WindowSession();
  const afterExecution = second.invoke(native, storeRequest("open", "{}"));
  await Promise.resolve();
  assert.equal(count, 1);
  const secondDrain = second.retire();
  callbacks.shift()?.(opened("shared"));
  await afterExecution;
  await secondDrain;
  assert.equal(count, 0);
  assert.equal(closeCalls, 2);
});

test("an explicit close in flight is not repeated by retirement", async () => {
  let count = 0;
  let closeCalls = 0;
  let completeClose: ((raw: string) => void) | undefined;
  const native = {
    store: {
      open: (_request: string, callback: (raw: string) => void) => {
        count++;
        callback(opened("shared"));
      },
      close: (_spaceId: string, callback: (raw: string) => void) => {
        closeCalls++;
        completeClose = (raw) => {
          if (JSON.parse(raw).ok) count--;
          callback(raw);
        };
      },
    },
  };
  const session = new WindowSession();
  await session.invoke(native, storeRequest("open", "{}"));
  const close = session.invoke(native, storeRequest("close", "shared"));
  await Promise.resolve();
  const drain = session.retire();
  completeClose?.(closed);
  assert.equal(JSON.parse(await close).ok, true);
  await drain;
  assert.equal(count, 0);
  assert.equal(closeCalls, 1);
});

test("failed opens add no handle and failed closes remain owned", async () => {
  let closeCalls = 0;
  const diagnostics: string[] = [];
  const native = {
    store: {
      open: (request: string, callback: (raw: string) => void) =>
        callback(
          JSON.parse(request).fail
            ? '{"ok":false,"error":{"reason":"FAILED"}}'
            : opened("shared"),
        ),
      close: (_spaceId: string, callback: (raw: string) => void) => {
        closeCalls++;
        callback(
          closeCalls === 1
            ? '{"ok":false,"error":{"reason":"FAILED"}}'
            : closed,
        );
      },
    },
  };
  const session = new WindowSession((category) => diagnostics.push(category));
  assert.equal(
    JSON.parse(
      await session.invoke(native, storeRequest("open", '{"fail":true}')),
    ).ok,
    false,
  );
  await session.invoke(native, storeRequest("open", "{}"));
  assert.equal(
    JSON.parse(await session.invoke(native, storeRequest("close", "shared")))
      .ok,
    false,
  );
  await session.retire();
  assert.equal(closeCalls, 2);
  assert.deepEqual(diagnostics, ["native_close_failed"]);
});
