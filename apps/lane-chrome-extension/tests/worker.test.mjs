// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
const execFileAsync = promisify(execFile);
function event() {
  const listeners = [];
  return {
    addListener: (fn) => listeners.push(fn),
    emit: (...args) => listeners.map((fn) => fn(...args)),
    count: () => listeners.length,
  };
}
test("worker binds a selection to the approved normal active tab and sends only cropped PNG", async () => {
  const messages = event(),
    nativeMessages = event(),
    storage = {},
    session = {},
    sent = [];
  let moved = false,
    changeDuringCapture = false;
  let active = 1,
    restricted = false,
    accessLevel,
    focused = true;
  const activations = [];
  const tab = () => ({
    id: 1,
    windowId: 7,
    url: restricted ? "chrome://settings" : "https://example.com/test",
    incognito: false,
  });
  const port = {
    onMessage: nativeMessages,
    onDisconnect: event(),
    postMessage(message) {
      sent.push(message);
      queueMicrotask(() =>
        nativeMessages.emit({
          id: message.id,
          ok: true,
          ...(message.type === "connect" ? { token: "credential" } : {}),
        }),
      );
    },
    disconnect() {},
  };
  globalThis.chrome = {
    runtime: {
      id: "a".repeat(32),
      onMessage: messages,
      connectNative: () => port,
    },
    storage: {
      local: {
        setAccessLevel: async (value) => (accessLevel = value.accessLevel),
        get: async () => ({ ...storage }),
        set: async (value) => Object.assign(storage, value),
        remove: async (key) => delete storage[key],
      },
      session: { set: async (value) => Object.assign(session, value) },
    },
    action: { setBadgeText: async () => {}, setTitle: async () => {} },
    tabs: {
      query: async () => [{ ...tab(), id: active }],
      onRemoved: event(),
      captureVisibleTab: async () => {
        if (changeDuringCapture) moved = true;
        return "data:image/png;base64,AQID";
      },
    },
    windows: {
      get: async () => ({ focused, state: focused ? "normal" : "minimized" }),
      update: async (id, options) => {
        activations.push({ id, ...options });
        focused = true;
      },
    },
    scripting: {
      executeScript: async () => [
        {
          result: {
            url: "https://example.com/test",
            width: 100,
            height: 50,
            scrollX: 0,
            scrollY: moved ? 1 : 0,
          },
        },
      ],
    },
    commands: { onCommand: event() },
  };
  let crop;
  globalThis.createImageBitmap = async () => ({
    width: 200,
    height: 100,
    close() {},
  });
  globalThis.OffscreenCanvas = class {
    constructor(width, height) {
      this.width = width;
      this.height = height;
    }
    getContext() {
      return { drawImage: (...args) => (crop = args.slice(1)) };
    }
    async convertToBlob() {
      return new Blob([new Uint8Array([4, 5, 6])], { type: "image/png" });
    }
  };
  await import(`../src/worker.js?test=${Date.now()}`);
  const invoke = (message, sender = { id: chrome.runtime.id }) =>
    new Promise((resolve) => messages.emit(message, sender, resolve));
  assert.equal(accessLevel, "TRUSTED_CONTEXTS");
  assert.equal(
    (await invoke({ type: "connect", profileName: "Work" })).ok,
    true,
  );
  assert.equal(storage.token, "credential");
  assert.equal((await invoke({ type: "capture" })).ok, true);
  const selection = {
    type: "selection",
    rect: { x: -5, y: 5, width: 20, height: 30 },
    viewport: { width: 100, height: 50, scrollX: 0, scrollY: 0 },
    metadata: {
      pageUrl: "https://example.com/test",
      pageTitle: "Literal <b> title",
      cssSelector: "#target",
    },
  };
  assert.equal(
    (await invoke(selection, { tab: tab(), frameId: 0, id: chrome.runtime.id }))
      .ok,
    true,
  );
  const capture = sent.find((value) => value.type === "capture");
  assert.equal(capture.pngBase64, "BAUG");
  assert.equal(capture.width, 30);
  assert.equal(capture.height, 60);
  assert.equal(capture.clipped, true);
  assert.equal(capture.token, "credential");
  assert.deepEqual(crop, [0, 10, 30, 60, 0, 0, 30, 60]);
  assert.equal(
    (await invoke(selection, { tab: tab(), frameId: 0, id: chrome.runtime.id }))
      .ok,
    false,
    "cannot replay selection",
  );
  await invoke({ type: "capture" });
  active = 2;
  assert.equal(
    (await invoke(selection, { tab: tab(), frameId: 0, id: chrome.runtime.id }))
      .ok,
    false,
    "tab switch cancels capture",
  );
  active = 1;
  restricted = true;
  assert.equal((await invoke({ type: "capture" })).ok, false);
  restricted = false;
  await invoke({ type: "capture" });
  changeDuringCapture = true;
  assert.equal(
    (await invoke(selection, { tab: tab(), frameId: 0, id: chrome.runtime.id }))
      .ok,
    false,
    "page scroll during screenshot cancels capture",
  );
  assert.equal(sent.filter((value) => value.type === "capture").length, 1);
  assert.equal(JSON.stringify(storage).includes("Literal"), false);
  focused = false;
  assert.equal(
    (await invoke({ type: "capture" })).ok,
    false,
    "unfocused shortcut/popup does not activate Chrome",
  );
  assert.equal(activations.length, 0);
  nativeMessages.emit({
    type: "select",
    selectionId: "from-lane",
    activate: true,
  });
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(activations, [{ id: 7, focused: true, state: "normal" }]);
  assert.equal(
    sent.find((value) => value.selectionId === "from-lane").reason,
    undefined,
  );
  restricted = true;
  nativeMessages.emit({
    type: "select",
    selectionId: "restricted",
    activate: true,
  });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(
    sent.find((value) => value.selectionId === "restricted").reason,
    "UNSUPPORTED_PAGE",
  );
  assert.equal(activations.length, 1, "restricted pages are not activated");
  delete globalThis.chrome;
  delete globalThis.createImageBitmap;
  delete globalThis.OffscreenCanvas;
});

// Shared mocked-worker harness for the startup-ordering and URL-guard tests below.
// Every chrome API call that touches credentials, native traffic, screenshots or
// selector injection is counted so tests can assert that it did not happen.
let imports = 0;
function createHarness({
  setAccessLevel = async () => {},
  tab: tabOverrides = {},
  executeScript = async () => [],
} = {}) {
  const messages = event(),
    nativeMessages = event(),
    commands = event(),
    removed = event(),
    storage = {},
    session = {},
    sent = [],
    activations = [],
    calls = {
      storageLocal: 0,
      connectNative: 0,
      executeScript: 0,
      captureVisibleTab: 0,
      tabsQuery: 0,
    };
  let tabState = {
    id: 1,
    windowId: 7,
    url: "https://example.com/test",
    incognito: false,
    ...tabOverrides,
  };
  let tabPresent = true;
  const port = {
    onMessage: nativeMessages,
    onDisconnect: event(),
    postMessage(message) {
      sent.push(message);
      queueMicrotask(() =>
        nativeMessages.emit({
          id: message.id,
          ok: true,
          ...(message.type === "connect" ? { token: "credential" } : {}),
        }),
      );
    },
    disconnect() {},
  };
  globalThis.chrome = {
    runtime: {
      id: "a".repeat(32),
      onMessage: messages,
      connectNative: () => {
        calls.connectNative++;
        return port;
      },
    },
    storage: {
      local: {
        setAccessLevel,
        get: async () => {
          calls.storageLocal++;
          return { ...storage };
        },
        set: async (value) => {
          calls.storageLocal++;
          Object.assign(storage, value);
        },
        remove: async (key) => {
          calls.storageLocal++;
          delete storage[key];
        },
      },
      session: { set: async (value) => Object.assign(session, value) },
    },
    action: { setBadgeText: async () => {}, setTitle: async () => {} },
    tabs: {
      query: async () => {
        calls.tabsQuery++;
        return tabPresent ? [{ ...tabState }] : [];
      },
      onRemoved: removed,
      captureVisibleTab: async () => {
        calls.captureVisibleTab++;
        return "data:image/png;base64,AQID";
      },
    },
    windows: {
      get: async () => ({ focused: true, state: "normal" }),
      update: async (id, options) => {
        activations.push({ id, ...options });
      },
    },
    scripting: {
      executeScript: async (...args) => {
        calls.executeScript++;
        return executeScript(...args);
      },
    },
    commands: { onCommand: commands },
  };
  const invoke = (message, sender = { id: chrome.runtime.id }) =>
    new Promise((resolve) => messages.emit(message, sender, resolve));
  return {
    messages,
    nativeMessages,
    commands,
    removed,
    storage,
    session,
    sent,
    activations,
    calls,
    invoke,
    setTab(value) {
      tabState = { ...tabState, ...value };
    },
    omitTabUrl() {
      delete tabState.url;
    },
    removeTab() {
      tabPresent = false;
    },
    load: () => import(`../src/worker.js?harness=${Date.now()}-${++imports}`),
    // Connects through the popup path so the native port exists and tokens are stored.
    connect: () => invoke({ type: "connect", profileName: "Work" }),
    // Emits a native select request and returns the resulting selectionResult message.
    async nativeSelect(selectionId, activate) {
      nativeMessages.emit({ type: "select", selectionId, activate });
      await settle();
      return sent.find(
        (value) =>
          value.type === "selectionResult" && value.selectionId === selectionId,
      );
    },
    dispose() {
      delete globalThis.chrome;
    },
  };
}
async function settle() {
  for (let i = 0; i < 10; i++) await new Promise((r) => setImmediate(r));
}
const selectionMessage = {
  type: "selection",
  rect: { x: 0, y: 0, width: 10, height: 10 },
  viewport: { width: 100, height: 50, scrollX: 0, scrollY: 0 },
  metadata: { pageUrl: "https://example.com/test", cssSelector: "#target" },
};
const noProtectedWork = (calls) => {
  assert.equal(calls.storageLocal, 0, "no credential storage access");
  assert.equal(calls.connectNative, 0, "no native traffic");
  assert.equal(calls.executeScript, 0, "no selector injection");
  assert.equal(calls.captureVisibleTab, 0, "no screenshot");
  assert.equal(calls.tabsQuery, 0, "no tab inspection");
};

test("worker module graph has no top-level await", async () => {
  // Chromium rejects service workers whose module graph is asynchronous, while Node
  // happily imports such modules. Detect top-level await statically instead.
  const script = `
    import { SourceTextModule } from "node:vm";
    import { readFile } from "node:fs/promises";
    import { fileURLToPath } from "node:url";
    const seen = new Set(), offenders = [];
    async function visit(url) {
      if (seen.has(url.href)) return;
      seen.add(url.href);
      const source = await readFile(url, "utf8");
      if (new SourceTextModule(source).hasTopLevelAwait()) offenders.push(fileURLToPath(url));
      for (const match of source.matchAll(/from\\s+"(\\.[^"]+)"/g))
        await visit(new URL(match[1], url));
    }
    await visit(new URL(process.argv[1]));
    console.log(JSON.stringify({ visited: seen.size, offenders }));
  `;
  const { stdout } = await execFileAsync(
    process.execPath,
    [
      "--experimental-vm-modules",
      "--no-warnings",
      "--input-type=module",
      "-e",
      script,
      new URL("../src/worker.js", import.meta.url).href,
    ],
    { encoding: "utf8" },
  );
  const result = JSON.parse(stdout);
  assert.ok(result.visited >= 2, "worker and protocol modules were inspected");
  assert.deepEqual(result.offenders, []);
});

test("listeners register synchronously and protected work waits for pending access initialization", async () => {
  let release;
  const harness = createHarness({
    setAccessLevel: () => new Promise((resolve) => (release = resolve)),
  });
  try {
    // The import itself must complete while initialization is still pending.
    await harness.load();
    assert.equal(harness.messages.count(), 1);
    assert.equal(harness.commands.count(), 1);
    assert.equal(harness.removed.count(), 1);
    const connect = harness.invoke({ type: "connect", profileName: "Work" });
    const capture = harness.invoke({ type: "capture" });
    const selection = harness.invoke(selectionMessage, {
      tab: { id: 1, windowId: 7, url: "https://example.com/test" },
      frameId: 0,
      id: chrome.runtime.id,
    });
    harness.commands.emit("select-element");
    let replies = 0;
    for (const reply of [connect, capture, selection])
      reply.then(() => replies++);
    await settle();
    assert.equal(replies, 0, "operations stay pending");
    noProtectedWork(harness.calls);
    release();
    const [connected, captureReply, selectionReply] = await Promise.all([
      connect,
      capture,
      selection,
    ]);
    assert.equal(connected.ok, true);
    assert.equal(harness.storage.token, "credential");
    assert.equal(captureReply.ok, true);
    assert.equal(
      harness.calls.executeScript >= 1,
      true,
      "selector injected after init",
    );
    assert.equal(selectionReply.ok, false, "no pending selection to replay");
    assert.equal(harness.calls.captureVisibleTab, 0);
  } finally {
    harness.dispose();
  }
});

test("rejected access initialization is handled and fails closed", async () => {
  const unhandled = [];
  const record = (reason) => unhandled.push(reason);
  process.on("unhandledRejection", record);
  const logged = [];
  const originalError = console.error;
  console.error = (...args) => logged.push(args);
  const harness = createHarness({
    setAccessLevel: async () => {
      throw new Error("storage unavailable");
    },
  });
  try {
    await harness.load();
    await settle();
    assert.equal(harness.messages.count(), 1);
    assert.equal(harness.commands.count(), 1);
    assert.equal(harness.removed.count(), 1);
    const connect = await harness.invoke({
      type: "connect",
      profileName: "Work",
    });
    const capture = await harness.invoke({ type: "capture" });
    const selection = await harness.invoke(selectionMessage, {
      tab: { id: 1, windowId: 7, url: "https://example.com/test" },
      frameId: 0,
      id: chrome.runtime.id,
    });
    for (const reply of [connect, capture, selection]) {
      assert.equal(reply.ok, false);
      assert.match(reply.message, /could not secure RealQA's local storage/);
    }
    harness.commands.emit("select-element");
    await settle();
    assert.match(
      harness.session.status,
      /could not secure RealQA's local storage/,
    );
    noProtectedWork(harness.calls);
    assert.deepEqual(harness.storage, {});
    assert.equal(logged.length, 1, "initialization failure is logged once");
    await settle();
    assert.deepEqual(unhandled, [], "no unhandled rejection");
  } finally {
    console.error = originalError;
    process.off("unhandledRejection", record);
    harness.dispose();
  }
});

test("native revocation removes the stored token after successful initialization", async () => {
  const harness = createHarness();
  try {
    await harness.load();
    assert.equal((await harness.connect()).ok, true);
    assert.equal(harness.storage.token, "credential");
    harness.nativeMessages.emit({ type: "revoked" });
    await settle();
    assert.equal(harness.storage.token, undefined);
    assert.match(harness.session.status, /disconnected/);
  } finally {
    harness.dispose();
  }
});
