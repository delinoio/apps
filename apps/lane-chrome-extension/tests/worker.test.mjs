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

async function connectedHarness(options) {
  const harness = createHarness(options);
  await harness.load();
  assert.equal((await harness.connect()).ok, true);
  harness.sent.length = 0;
  harness.calls.executeScript = 0;
  harness.calls.captureVisibleTab = 0;
  return harness;
}
const clickExtensionRemedy =
  "Click the RealQA extension on this page, then select an element.";

test("native selection requests with an unknown tab URL ask for a tab grant", async () => {
  for (const omit of ["omitted", "empty"]) {
    for (const activate of [false, true]) {
      const harness = await connectedHarness();
      try {
        if (omit === "omitted") harness.omitTabUrl();
        else harness.setTab({ url: "" });
        const result = await harness.nativeSelect(
          `unknown-${activate}`,
          activate,
        );
        assert.equal(
          result.reason,
          "PERMISSION_REQUIRED",
          `${omit}/${activate}`,
        );
        assert.equal(harness.session.status, clickExtensionRemedy);
        assert.equal(harness.calls.executeScript, 0, "no injection");
        assert.equal(harness.calls.captureVisibleTab, 0, "no capture");
        assert.deepEqual(harness.activations, [], "no activation");
        assert.equal(
          harness.sent.some((value) => value.type === "status"),
          false,
          "no native status request",
        );
      } finally {
        harness.dispose();
      }
    }
  }
});

test("popup and shortcut selection with an unknown tab URL report the click-extension remedy", async () => {
  const harness = await connectedHarness();
  try {
    harness.omitTabUrl();
    const reply = await harness.invoke({ type: "capture" });
    assert.deepEqual(reply, { ok: false, message: clickExtensionRemedy });
    harness.commands.emit("select-element");
    await settle();
    assert.equal(harness.session.status, clickExtensionRemedy);
    assert.equal(harness.calls.executeScript, 0);
    assert.equal(harness.calls.captureVisibleTab, 0);
  } finally {
    harness.dispose();
  }
});

test("known restricted, incognito and absent tabs remain unsupported without injection", async () => {
  const cases = [
    ["chrome internal", { url: "chrome://settings" }],
    ["file", { url: "file:///tmp/example.html" }],
    [
      "current Web Store",
      { url: "https://chromewebstore.google.com/detail/x" },
    ],
    [
      "legacy Web Store",
      { url: "https://chrome.google.com/webstore/detail/x" },
    ],
    ["incognito", { url: "https://example.com/test", incognito: true }],
    ["incognito without url", { url: undefined, incognito: true }],
  ];
  for (const activate of [false, true]) {
    for (const [name, tab] of cases) {
      const harness = await connectedHarness();
      try {
        harness.setTab(tab);
        const result = await harness.nativeSelect(`case-${activate}`, activate);
        assert.equal(result.reason, "UNSUPPORTED_PAGE", name);
        assert.equal(harness.calls.executeScript, 0, name);
        assert.equal(harness.calls.captureVisibleTab, 0, name);
        assert.deepEqual(harness.activations, [], name);
      } finally {
        harness.dispose();
      }
    }
    const harness = await connectedHarness();
    try {
      harness.removeTab();
      const result = await harness.nativeSelect(`absent-${activate}`, activate);
      assert.equal(result.reason, "UNSUPPORTED_PAGE", "no active tab");
      assert.equal(harness.calls.executeScript, 0);
      assert.equal(harness.calls.captureVisibleTab, 0);
    } finally {
      harness.dispose();
    }
  }
});

test("known ordinary tabs inject the selector and map denied injection to a tab grant request", async () => {
  for (const url of ["http://example.com/test", "https://example.com/test"]) {
    const granted = await connectedHarness();
    try {
      granted.setTab({ url });
      const result = await granted.nativeSelect("granted", false);
      assert.equal(result.reason, undefined, url);
      assert.equal(granted.calls.executeScript, 1, url);
    } finally {
      granted.dispose();
    }
    const denied = await connectedHarness({
      executeScript: async () => {
        throw new Error("Cannot access contents of the page");
      },
    });
    try {
      denied.setTab({ url });
      const result = await denied.nativeSelect("denied", false);
      assert.equal(result.reason, "PERMISSION_REQUIRED", url);
      assert.equal(denied.session.status, clickExtensionRemedy);
      assert.equal(denied.calls.captureVisibleTab, 0, "no capture on failure");
    } finally {
      denied.dispose();
    }
  }
});

test("the manifest still requests no tabs or host permissions", async () => {
  const { readFile } = await import("node:fs/promises");
  const manifest = JSON.parse(
    await readFile(new URL("../src/manifest.json", import.meta.url), "utf8"),
  );
  assert.equal(manifest.host_permissions, undefined);
  assert.equal(manifest.permissions.includes("tabs"), false);
  assert.equal(manifest.permissions.includes("<all_urls>"), false);
});

test("actual Web Store URLs are rejected before status and injection", async () => {
  const urls = [
    "https://chromewebstore.google.com/",
    "https://chromewebstore.google.com/detail/item/abc",
    "https://chromewebstore.google.com/detail/item/abc?hl=en#reviews",
    "https://chrome.google.com/webstore",
    "https://chrome.google.com/webstore?hl=en#top",
    "https://chrome.google.com/webstore/detail/item",
    "https://chrome.google.com/webstore/detail/item?hl=en#reviews",
  ];
  for (const url of urls) {
    const harness = await connectedHarness();
    try {
      harness.setTab({ url });
      const result = await harness.nativeSelect("store", true);
      assert.equal(result.reason, "UNSUPPORTED_PAGE", url);
      assert.equal(
        harness.sent.some((value) => value.type === "status"),
        false,
        `${url} sends no status request`,
      );
      assert.equal(harness.calls.executeScript, 0, `${url} is not injected`);
      assert.deepEqual(harness.activations, [], url);
    } finally {
      harness.dispose();
    }
  }
});

test("Web Store lookalikes and sibling paths follow the normal page path", async () => {
  const urls = [
    "https://chrome.google.com/webstorefront",
    "https://chrome.google.com.example.org/webstore",
    "https://chrome-google-com.example.org/webstore",
    "https://chromewebstore.google.com.example.org/test",
    "https://chromewebstore-google-com.example.org/test",
    "http://example.com/test",
    "https://example.com/test",
  ];
  for (const url of urls) {
    const harness = await connectedHarness();
    try {
      harness.setTab({ url });
      const result = await harness.nativeSelect("ordinary", false);
      assert.equal(result.reason, undefined, url);
      assert.equal(
        harness.sent.filter((value) => value.type === "status").length,
        1,
        `${url} requests native status`,
      );
      assert.equal(harness.calls.executeScript, 1, `${url} is injected`);
    } finally {
      harness.dispose();
    }
  }
});

test("incognito, internal, file, absent and malformed URLs are rejected safely", async () => {
  const cases = [
    ["incognito", { url: "https://example.com/test", incognito: true }],
    ["internal", { url: "chrome://settings" }],
    ["file", { url: "file:///tmp/example.html" }],
    ["malformed", { url: "https://" }],
    ["not a URL", { url: "not a url" }],
    ["non-string", { url: 42 }],
  ];
  for (const [name, tab] of cases) {
    const harness = await connectedHarness();
    try {
      harness.setTab(tab);
      const result = await harness.nativeSelect("rejected", false);
      assert.equal(result.reason, "UNSUPPORTED_PAGE", name);
      assert.equal(harness.calls.executeScript, 0, name);
      assert.equal(harness.calls.captureVisibleTab, 0, name);
    } finally {
      harness.dispose();
    }
  }
  // An absent URL is unknown rather than malformed: it asks for a tab grant and
  // still performs no status request or injection.
  const absent = await connectedHarness();
  try {
    absent.omitTabUrl();
    const result = await absent.nativeSelect("absent", false);
    assert.equal(result.reason, "PERMISSION_REQUIRED");
    assert.equal(absent.calls.executeScript, 0);
  } finally {
    absent.dispose();
  }
});
