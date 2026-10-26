// SPDX-License-Identifier: Apache-2.0
import {
  HOST,
  isSelectablePageUrl,
  safeMetadata,
  visibleRect,
  remedies,
} from "./protocol.js";
// Chromium rejects service workers whose module graph uses top-level await, so the
// trusted-context restriction is started here without awaiting it. Every handler is
// registered synchronously below, and every operation that touches credentials,
// native messaging, screenshots or selector injection first waits on `accessReady`.
const accessReady = chrome.storage.local.setAccessLevel({
  accessLevel: "TRUSTED_CONTEXTS",
});
// Handle the rejection on a side branch so a failed initialization is never reported
// as an unhandled rejection. `accessReady` itself stays rejected, which keeps
// protected work failing closed until Chrome restarts the worker.
accessReady.catch((error) => {
  console.error("lane-realqa: trusted-context storage initialization failed", {
    reason: String(error?.message || error),
  });
});
async function ensureAccess() {
  try {
    await accessReady;
  } catch {
    throw new Error("ACCESS_UNAVAILABLE");
  }
}
let port,
  serial = 0;
const pending = new Map(),
  selections = new Map();
async function showStatus(message) {
  await chrome.storage.session.set({ status: message });
  await chrome.action.setBadgeText({ text: "!" });
  await chrome.action.setTitle({ title: message });
}
function connection() {
  if (port) return port;
  port = chrome.runtime.connectNative(HOST);
  port.onMessage.addListener((message) => {
    if (message.type === "revoked") {
      void ensureAccess()
        .then(() => chrome.storage.local.remove("token"))
        .catch(() => {});
      void showStatus(remedies.REVOKED);
      return;
    }
    if (message.type === "select") {
      void select({ activate: message.activate === true })
        .then(
          () =>
            request("selectionResult", { selectionId: message.selectionId }),
          async (error) => {
            await showStatus(remedies[error.message] || error.message);
            await request("selectionResult", {
              selectionId: message.selectionId,
              reason: error.message,
            });
          },
        )
        .catch(() => {});
      return;
    }
    const call = pending.get(message.id);
    if (!call) return;
    pending.delete(message.id);
    clearTimeout(call.timer);
    message.ok
      ? call.resolve(message)
      : call.reject(new Error(message.reason || "CONNECTION_FAILED"));
  });
  port.onDisconnect.addListener(() => {
    void chrome.runtime.lastError;
    port = undefined;
    for (const call of pending.values()) {
      clearTimeout(call.timer);
      call.reject(new Error("LANE_NOT_RUNNING"));
    }
    pending.clear();
  });
  return port;
}
async function identity() {
  await ensureAccess();
  let value = await chrome.storage.local.get([
    "profileId",
    "profileName",
    "token",
  ]);
  if (!value.profileId) {
    value.profileId = crypto.randomUUID();
    await chrome.storage.local.set({ profileId: value.profileId });
  }
  return value;
}
async function request(type, body = {}) {
  await ensureAccess();
  const credentials = await identity();
  const p = connection(),
    id = String(++serial);
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error("CONNECTION_TIMEOUT"));
    }, 120000);
    pending.set(id, { resolve, reject, timer });
    p.postMessage({ id, type, ...credentials, ...body });
  });
}
async function select({ activate = false } = {}) {
  await ensureAccess();
  const [tab] = await chrome.tabs.query({
    active: true,
    lastFocusedWindow: true,
  });
  const win = tab && (await chrome.windows.get(tab.windowId));
  if (!tab || tab.incognito) throw new Error("UNSUPPORTED_PAGE");
  // Without a grant for this tab Chrome hides tab.url, so an omitted or empty URL is
  // unknown rather than restricted. Ask the user to click the extension and never
  // inject, capture or activate Chrome for it.
  if (tab.url === undefined || tab.url === null || tab.url === "")
    throw new Error("PERMISSION_REQUIRED");
  if (!isSelectablePageUrl(tab.url)) throw new Error("UNSUPPORTED_PAGE");
  await request("status");
  if (activate) {
    await chrome.windows.update(tab.windowId, {
      focused: true,
      ...(win.state === "minimized" ? { state: "normal" } : {}),
    });
  }
  if (!(await chrome.windows.get(tab.windowId)).focused)
    throw new Error("CHROME_NOT_AVAILABLE");
  selections.set(tab.id, { url: tab.url, windowId: tab.windowId });
  try {
    await chrome.scripting.executeScript({
      target: { tabId: tab.id },
      files: ["selector.js"],
    });
  } catch {
    selections.delete(tab.id);
    throw new Error("PERMISSION_REQUIRED");
  }
}
async function capture(message, sender) {
  const tab = sender.tab,
    selection = tab && selections.get(tab.id);
  selections.delete(tab?.id);
  if (
    !selection ||
    sender.frameId !== 0 ||
    tab.incognito ||
    tab.url !== selection.url
  )
    throw new Error("UNSUPPORTED_PAGE");
  const [active] = await chrome.tabs.query({
    active: true,
    windowId: tab.windowId,
  });
  const win = await chrome.windows.get(tab.windowId);
  if (active?.id !== tab.id || !win.focused) throw new Error("PAGE_CHANGED");
  const metadata = safeMetadata(message.metadata);
  if (metadata.pageUrl !== selection.url) throw new Error("PAGE_CHANGED");
  async function verifyPage() {
    const results = await chrome.scripting.executeScript({
      target: { tabId: tab.id },
      func: () => ({
        url: location.href,
        width: innerWidth,
        height: innerHeight,
        scrollX,
        scrollY,
      }),
    });
    const current = results[0]?.result;
    if (
      !current ||
      current.url !== metadata.pageUrl ||
      ["width", "height", "scrollX", "scrollY"].some(
        (key) => current[key] !== message.viewport[key],
      )
    )
      throw new Error("PAGE_CHANGED");
    const [selected] = await chrome.tabs.query({
      active: true,
      windowId: tab.windowId,
    });
    if (
      selected?.id !== tab.id ||
      !(await chrome.windows.get(tab.windowId)).focused
    )
      throw new Error("PAGE_CHANGED");
  }
  await verifyPage();
  const rect = visibleRect(message.rect, message.viewport);
  const data = await chrome.tabs.captureVisibleTab(tab.windowId, {
    format: "png",
  });
  await verifyPage();
  // Decode and crop entirely in worker memory; never persist the complete viewport.
  const bitmap = await createImageBitmap(await (await fetch(data)).blob());
  const sx = bitmap.width / message.viewport.width,
    sy = bitmap.height / message.viewport.height;
  const x = Math.ceil(rect.x * sx),
    y = Math.ceil(rect.y * sy),
    right = Math.floor((rect.x + rect.width) * sx),
    bottom = Math.floor((rect.y + rect.height) * sy);
  if (right <= x || bottom <= y) {
    bitmap.close();
    throw new Error("ELEMENT_NOT_VISIBLE");
  }
  const canvas = new OffscreenCanvas(right - x, bottom - y);
  canvas
    .getContext("2d")
    .drawImage(
      bitmap,
      x,
      y,
      right - x,
      bottom - y,
      0,
      0,
      right - x,
      bottom - y,
    );
  bitmap.close();
  const bytes = new Uint8Array(
    await (await canvas.convertToBlob({ type: "image/png" })).arrayBuffer(),
  );
  let binary = "";
  for (let i = 0; i < bytes.length; i += 8192)
    binary += String.fromCharCode(...bytes.subarray(i, i + 8192));
  await request("capture", {
    pngBase64: btoa(binary),
    width: canvas.width,
    height: canvas.height,
    metadata,
    clipped: rect.clipped || message.clipped === true,
  });
  await chrome.action.setBadgeText({ text: "" });
  return {
    ok: true,
    message: "Capture added to Lane. Review metadata before saving.",
  };
}
chrome.runtime.onMessage.addListener((message, sender, reply) => {
  // Popup commands are accepted only from this extension, never from a page script.
  const operation = async () => {
    await ensureAccess();
    if (message.type === "selection") return capture(message, sender);
    if (sender.tab || sender.id !== chrome.runtime.id)
      throw new Error("UNSUPPORTED_REQUEST");
    if (message.type === "connect") {
      await chrome.storage.local.set({
        profileName: String(message.profileName || "Chrome profile").slice(
          0,
          80,
        ),
      });
      const result = await request("connect");
      await chrome.storage.local.set({ token: result.token });
      return { ok: true, message: "Connected. Select an element to capture." };
    }
    if (message.type === "disconnect") {
      await request("revoke");
      await chrome.storage.local.remove("token");
      port?.disconnect();
      return {
        ok: true,
        message: "Disconnected. Lane approval is required to reconnect.",
      };
    }
    if (message.type === "capture") {
      await select();
      return {
        ok: true,
        message: "Select an element on the page. Esc cancels.",
      };
    }
    throw new Error("UNSUPPORTED_REQUEST");
  };
  void operation().then(reply, (error) => {
    const message =
      remedies[error.message] ||
      "Capture failed. Your Lane draft is unchanged. Retry manually.";
    void showStatus(message);
    reply({ ok: false, message });
  });
  return true;
});
chrome.commands.onCommand.addListener((command) => {
  if (command === "select-element")
    void select().catch((error) =>
      showStatus(
        remedies[error.message] ||
          "Capture unavailable. Click RealQA on this page.",
      ),
    );
});
chrome.tabs.onRemoved.addListener((id) => selections.delete(id));
// A native port keeps the worker alive while Lane runs. Reconnect only on explicit
// user action after Lane quits; never launch the app or retry captures silently.
