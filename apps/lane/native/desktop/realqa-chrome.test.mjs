// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import net from "node:net";
import { once } from "node:events";
import { mkdtemp, readFile, rm, writeFile, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { RealQA } from "./realqa.mjs";
import { encodeMessage, readMessages } from "./chrome-framing.mjs";
import { connectChrome, ChromeRecovery } from "./realqa-chrome.mjs";
import { createChromeBridge } from "./chrome-bridge.mjs";
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
    await assert.rejects(host.capture(), /LANE_SAVING/);
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

test("worker PERMISSION_REQUIRED reaches click-to-grant guidance for in-app and shortcut triggers without losing the draft", async () => {
  const nativeCalls = [],
    selections = [];
  const host = new RealQA({
    native: {
      capture(raw, callback) {
        const request = JSON.parse(raw);
        nativeCalls.push(request.op);
        callback(JSON.stringify({ ok: true, value: { chrome: true } }));
      },
    },
  });
  host.extension = await connectChrome({
    realqa: host,
    createBridge: async () => ({
      startSelection: async (options) => {
        selections.push(options);
        throw new Error("PERMISSION_REQUIRED");
      },
    }),
  });
  host.draft = {
    id: "draft",
    images: [],
    title: "Keep",
    description: "",
    repositoryId: "r",
  };
  const draft = structuredClone(host.draft);
  for (const trigger of [undefined, "shortcut"]) {
    const reply = JSON.parse(
      await host.invoke(
        JSON.stringify({ op: "capture", mode: "dom", trigger }),
      ),
    );
    assert.equal(reply.ok, false);
    assert.equal(
      reply.error.message,
      "Click the RealQA extension on this Chrome tab to grant tab access, then retry.",
    );
    assert.equal(reply.error.retryable, true);
  }
  assert.deepEqual(selections, [{ activate: true }, { activate: false }]);
  assert.deepEqual(host.draft, draft, "draft is preserved");
  assert.deepEqual(
    nativeCalls,
    ["activeBrowser"],
    "only the shortcut foreground check ran and no fallback capture was attempted",
  );
});

test("actual bridge reconnect invalidates RealQA with connected state and no status request", async (t) => {
  const dataRoot = await mkdtemp(join(tmpdir(), "lane-realqa-reconnect-"));
  const extensionId = "a".repeat(32),
    profileId = "01900000-0000-7000-8000-000000000001";
  const snapshots = [];
  let host,
    approvals = 0;
  const sockets = [];
  const until = async (condition) => {
    for (let attempt = 0; attempt < 400; attempt += 1) {
      if (condition()) return;
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
    throw new Error("TIMEOUT");
  };
  const requests = [];
  async function client() {
    const runtime = JSON.parse(
      await readFile(join(dataRoot, "realqa-chrome-runtime.json"), "utf8"),
    );
    const socket = net.connect(runtime.endpoint);
    sockets.push(socket);
    await once(socket, "connect");
    const pending = new Map();
    let sequence = 0;
    readMessages(
      socket,
      (message) => pending.get(message.id)?.(message),
      () => {},
    );
    socket.write(
      encodeMessage({ type: "relay", secret: runtime.secret, extensionId }),
    );
    return {
      socket,
      request(type, token) {
        const id = String(++sequence);
        requests.push(type);
        return new Promise((resolve) => {
          pending.set(id, resolve);
          socket.write(
            encodeMessage({
              id,
              type,
              profileId,
              profileName: "Synthetic profile",
              ...(token ? { token } : {}),
            }),
          );
        });
      },
    };
  }
  t.after(async () => {
    for (const socket of sockets) socket.destroy();
    await host?.close();
    await rm(dataRoot, { recursive: true, force: true });
  });
  host = await connectChrome({
    dataRoot,
    extensionIds: [extensionId],
    showWindow() {
      approvals += 1;
    },
    realqa: {
      changed() {
        // The settings view re-reads host state when this invalidation arrives.
        if (host) snapshots.push(host.state());
      },
    },
  });
  const first = await client();
  const initial = first.request("connect");
  await until(() => host.state().pending.length === 1);
  await host.invoke({ op: "chrome.approve", id: host.state().pending[0].id });
  const granted = await initial;
  assert.equal(granted.ok, true);
  assert.equal(approvals, 1);
  assert.equal(snapshots.at(-1).state, "connected");
  first.socket.end();
  await until(() => snapshots.at(-1).state === "disconnected");
  const before = snapshots.length;
  requests.length = 0;

  const second = await client();
  const accepted = await second.request("connect", granted.token);
  assert.equal(accepted.ok, true);
  assert.equal(accepted.token, granted.token);
  assert.equal(approvals, 1, "no new approval prompt");
  assert.equal(snapshots.length, before + 1, "one invalidation");
  assert.equal(snapshots.at(-1).state, "connected");
  assert.equal(snapshots.at(-1).pending.length, 0);
  assert.equal(snapshots.at(-1).connections[0].connected, true);
  assert.deepEqual(requests, ["connect"], "no status or other request");
});

const extensionId = "a".repeat(32),
  syntheticProfile = "01900000-0000-7000-8000-000000000002",
  pngBase64 =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";
const until = async (condition) => {
  for (let attempt = 0; attempt < 400; attempt += 1) {
    if (condition()) return;
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  throw new Error("TIMEOUT");
};
// Synthetic relay client speaking the framed bridge protocol after the handshake.
async function relayClient(dataRoot, sockets) {
  const runtime = JSON.parse(
    await readFile(join(dataRoot, "realqa-chrome-runtime.json"), "utf8"),
  );
  const socket = net.connect(runtime.endpoint);
  sockets.push(socket);
  await once(socket, "connect");
  const pending = new Map();
  let sequence = 0;
  readMessages(
    socket,
    (message) => pending.get(message.id)?.(message),
    () => {},
  );
  socket.write(
    encodeMessage({ type: "relay", secret: runtime.secret, extensionId }),
  );
  return {
    socket,
    request(type, body = {}) {
      const id = String(++sequence);
      return new Promise((resolve) => {
        pending.set(id, resolve);
        socket.write(
          encodeMessage({
            id,
            type,
            profileId: syntheticProfile,
            profileName: "Synthetic profile",
            ...body,
          }),
        );
      });
    },
  };
}
async function bridgeHost(t, realqa, options = {}) {
  const dataRoot =
    options.dataRoot ?? (await mkdtemp(join(tmpdir(), "lane-realqa-chrome-")));
  const sockets = [];
  const host = await connectChrome({
    dataRoot,
    extensionIds: [extensionId],
    showWindow() {},
    realqa,
    ...options,
  });
  t.after(async () => {
    for (const socket of sockets) socket.destroy();
    await host.close();
    await rm(dataRoot, { recursive: true, force: true });
  });
  return { host, dataRoot, client: () => relayClient(dataRoot, sockets) };
}
async function approvedClient(host, client) {
  const connection = await client();
  const reply = connection.request("connect");
  await until(() => host.state().pending.length === 1);
  await host.invoke({ op: "chrome.approve", id: host.state().pending[0].id });
  const { token } = await reply;
  return { ...connection, token };
}

test("Web Store listing appears only for a configured allowed extension ID", async () => {
  const stub = { changed() {} };
  const fake = async () => ({ listConnections: () => [], async close() {} });
  const listing = async (extensionIds, listingExtensionId) =>
    (
      await connectChrome({
        realqa: stub,
        extensionIds,
        listingExtensionId,
        createBridge: fake,
      })
    ).state().listingUrl;
  const published = "b".repeat(32);
  assert.equal(
    await listing([extensionId, published], published),
    `https://chromewebstore.google.com/detail/${published}`,
  );
  assert.equal(await listing([extensionId], published), undefined);
  assert.equal(await listing([extensionId], ""), undefined);
  assert.equal(await listing(["z".repeat(32)], "z".repeat(32)), undefined);
});

test("closing the requesting connection removes its pending approval and blocks a late approval", async (t) => {
  let changes = 0;
  const { host, client } = await bridgeHost(t, {
    changed() {
      changes += 1;
    },
  });
  const first = await client();
  void first.request("connect");
  await until(() => host.state().pending.length === 1);
  const abandoned = host.state().pending[0].id;
  const before = changes;
  first.socket.destroy();
  await until(() => host.state().pending.length === 0);
  assert.ok(changes > before, "settings are invalidated");
  assert.equal(host.state().state, "disconnected");
  await assert.rejects(
    host.invoke({ op: "chrome.approve", id: abandoned }),
    /no longer pending/,
  );
  assert.deepEqual(host.state().connections, [], "no approval was stored");
  // Reconnecting asks again instead of reporting APPROVAL_PENDING.
  const second = await client();
  const reply = second.request("connect");
  await until(() => host.state().pending.length === 1);
  await host.invoke({ op: "chrome.approve", id: host.state().pending[0].id });
  const granted = await reply;
  assert.equal(granted.ok, true);
  assert.equal(host.state().state, "connected");
});

test("bridge startup failures report an error state with explicit recovery", async (t) => {
  const dataRoot = await mkdtemp(join(tmpdir(), "lane-realqa-chrome-"));
  await writeFile(
    join(dataRoot, "realqa-chrome-connections.json"),
    "{not json",
    { mode: 0o600 },
  );
  const events = [];
  const { host } = await bridgeHost(
    t,
    { changed() {} },
    { dataRoot, log: (event, result) => events.push([event, result]) },
  );
  const failed = host.state();
  assert.equal(failed.state, "error");
  assert.equal(failed.recovery, ChromeRecovery.ResetApprovals);
  assert.match(failed.message, /Reset Chrome approvals/);
  assert.doesNotMatch(failed.message, new RegExp(dataRoot));
  assert.deepEqual(events, [
    ["lane_chrome_connection_unavailable", "CHROME_APPROVALS_INVALID"],
  ]);
  await assert.rejects(host.startDOMCapture(), /Reset Chrome approvals/);
  await assert.rejects(host.invoke({ op: "chrome.retry" }), /Reset Chrome/);
  assert.equal(host.state().state, "error", "retry alone keeps the file");
  await host.invoke({ op: "chrome.resetApprovals" });
  assert.equal(host.state().state, "disconnected");
  assert.equal(
    (
      await stat(join(dataRoot, "realqa-chrome-connections.json.invalid"))
    ).isFile(),
    true,
    "the unreadable file is moved aside, not parsed",
  );
  await assert.rejects(
    host.invoke({ op: "chrome.resetApprovals" }),
    /do not need to be reset/,
  );
});

test("an endpoint already in use reports an error that retry recovers", async (t) => {
  const dataRoot = await mkdtemp(join(tmpdir(), "lane-realqa-chrome-"));
  const other = await createChromeBridge({
    dataRoot,
    extensionIds: [extensionId],
    requestApproval: async () => false,
    acceptCapture: async () => {},
  });
  const { host } = await bridgeHost(t, { changed() {} }, { dataRoot });
  assert.equal(host.state().state, "error");
  assert.equal(host.state().recovery, ChromeRecovery.Retry);
  assert.match(host.state().message, /Another copy of Lane/);
  await other.close();
  assert.deepEqual(await host.invoke({ op: "chrome.retry" }), {});
  assert.equal(host.state().state, "disconnected");
});

test("post-selection failures reach Lane as specific guidance without page content", async (t) => {
  const emitted = [];
  const logs = [];
  const realqa = {
    saving: false,
    changed() {},
    emit: (name, value) => emitted.push([name, value]),
    enqueue: (action) => action(),
  };
  const { host, client } = await bridgeHost(t, realqa, {
    log: (event, result) => logs.push([event, result]),
  });
  const connection = await approvedClient(host, client);
  const reported = await connection.request("captureFailed", {
    token: connection.token,
    reason: "PAGE_CHANGED",
  });
  assert.equal(reported.ok, true);
  assert.deepEqual(emitted.at(-1), [
    "lane.realqa.error",
    {
      reason: "ERROR_REASON_DEPENDENCY_UNAVAILABLE",
      message:
        "The Chrome page moved or changed during capture. Your draft is unchanged. Select the element again.",
      retryable: true,
    },
  ]);
  await connection.request("captureFailed", {
    token: connection.token,
    reason: "https://example.test/secret",
  });
  assert.match(emitted.at(-1)[1].message, /could not be added/);
  realqa.saving = true;
  const rejected = await connection.request("capture", {
    token: connection.token,
    pngBase64,
    width: 1,
    height: 1,
    metadata: {
      pageUrl: "https://example.test/page",
      pageTitle: "",
      cssSelector: "#a",
    },
  });
  assert.equal(rejected.reason, "LANE_SAVING");
  assert.equal(emitted.at(-1)[1].reason, "ERROR_REASON_CONFLICT");
  assert.match(emitted.at(-1)[1].message, /Wait for the save to finish/);
  const unauthenticated = await connection.request("captureFailed", {
    token: "wrong",
    reason: "PAGE_CHANGED",
  });
  assert.equal(unauthenticated.reason, "REVOKED");
  assert.equal(emitted.length, 3, "unauthenticated reports are ignored");
  assert.equal(JSON.stringify(logs).includes("example.test"), false);
});
