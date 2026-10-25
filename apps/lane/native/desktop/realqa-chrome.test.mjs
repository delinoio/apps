// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { RealQA } from "./realqa.mjs";
import { connectChrome } from "./realqa-chrome.mjs";
const deferred = () => {
  let resolve;
  const promise = new Promise((done) => {
    resolve = done;
  });
  return { promise, resolve };
};
const image = { png: "cGl4ZWxz", width: 2, height: 2 };
async function setup() {
  const entered = deferred(),
    render = deferred();
  const realqa = new RealQA({
    emit() {},
    showComposer() {},
    closeComposer() {},
    native: {
      capture(raw, callback) {
        entered.resolve();
        render.promise.then(() =>
          callback(JSON.stringify({ ok: true, value: image })),
        );
      },
    },
  });
  let callbacks,
    acceptance = Promise.resolve(),
    revoked = false;
  realqa.extension = await connectChrome({
    realqa,
    showWindow() {},
    createBridge: async (options) => {
      callbacks = options;
      return {
        listConnections: () => [],
        async revoke() {
          await acceptance.catch(() => {});
          revoked = true;
        },
        async close() {},
      };
    },
  });
  return {
    realqa,
    entered,
    render,
    get revoked() {
      return revoked;
    },
    capture() {
      acceptance = callbacks.acceptCapture({
        pngBase64: image.png,
        width: 2,
        height: 2,
      });
      return acceptance;
    },
  };
}
test(
  "disconnect waits for decoding, rejects revoked pixels, and leaves state responsive",
  { timeout: 2000 },
  async () => {
    const host = await setup();
    const capture = host.capture();
    const rejected = assert.rejects(capture, /REVOKED/);
    await host.entered.promise;
    const disconnect = host.realqa.invoke(
      JSON.stringify({ op: "chrome.disconnect", profileId: "profile" }),
    );
    let done = false;
    disconnect.then(() => {
      done = true;
    });
    await Promise.resolve();
    assert.equal(done, false);
    assert.equal(host.revoked, false);
    const state = host.realqa.invoke(JSON.stringify({ op: "state" }));
    host.render.resolve();
    await rejected;
    assert.equal(JSON.parse(await disconnect).ok, true);
    assert.equal(JSON.parse(await state).ok, true);
    assert.equal(host.revoked, true);
    assert.equal(host.realqa.draft, null);
  },
);
test(
  "browser acceptance serializes with export and respects the save freeze",
  { timeout: 2000 },
  async () => {
    const host = await setup();
    const capture = host.capture();
    await host.entered.promise;
    const updated = host.realqa.invoke(
      JSON.stringify({
        op: "updateDraft",
        title: "Capture",
        repositoryId: "repository",
      }),
    );
    const exported = host.realqa.invoke(JSON.stringify({ op: "export" }));
    host.render.resolve();
    await capture;
    await updated;
    const result = JSON.parse(await exported);
    assert.equal(result.ok, true);
    assert.equal(result.value.images.length, 1);
    await assert.rejects(host.capture(), /save to finish/);
    assert.equal(host.realqa.draft.images.length, 1);
  },
);

test("Lane DOM actions activate Chrome while shortcuts require foreground Chrome", async () => {
  let foreground = false;
  const selections = [];
  const host = new RealQA({
    native: {
      capture(raw, callback) {
        assert.equal(JSON.parse(raw).op, "activeBrowser");
        callback(JSON.stringify({ ok: true, value: { chrome: foreground } }));
      },
    },
  });
  host.extension = await connectChrome({
    realqa: host,
    createBridge: async () => ({
      startSelection: async (options) => selections.push(options),
    }),
  });
  for (const window of ["main", "composer"]) {
    const reply = JSON.parse(
      await host.invoke(JSON.stringify({ op: "capture", mode: "dom" }), window),
    );
    assert.equal(reply.ok, true);
  }
  assert.deepEqual(selections, [{ activate: true }, { activate: true }]);
  const shortcut = JSON.stringify({
    op: "capture",
    mode: "dom",
    trigger: "shortcut",
  });
  assert.equal(JSON.parse(await host.invoke(shortcut)).ok, false);
  assert.equal(selections.length, 2);
  foreground = true;
  assert.equal(JSON.parse(await host.invoke(shortcut)).ok, true);
  assert.deepEqual(selections[2], { activate: false });
});
