// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
function event() {
  const listeners = [];
  return {
    addListener: (fn) => listeners.push(fn),
    emit: (...args) => listeners.map((fn) => fn(...args)),
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
