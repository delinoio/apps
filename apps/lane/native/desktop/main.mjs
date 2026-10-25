// SPDX-License-Identifier: Apache-2.0
import {
  app,
  LynxWindow,
  Menu,
  Tray,
  nativeImage,
  powerMonitor,
  lynxBridge,
} from "@lynx-js/lynxtron";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { control, WindowSession, unavailable } from "./bridge.mjs";
import { RealQA } from "./realqa.mjs";
import { connectChrome } from "./realqa-chrome.mjs";
import { homedir } from "node:os";
import { realQASmoke } from "./realqa-smoke.mjs";

const resources = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const { scheme, chromeExtensionIds = [] } = require("./package.json");
const native = require("./lane_host.node");
let window, windowSession, tray, poller, composer, realqa;
let captureWindows = [];
const pendingRealQA = [];
const emitRealQA = (name, value) => {
  if (
    ["lane.realqa.select", "lane.realqa.saved", "lane.realqa.error"].includes(
      name,
    ) &&
    !windowSession?.ready
  ) {
    pendingRealQA.push([name, value]);
    return;
  }
  for (const owner of [window, composer])
    if (owner && !owner.isDestroyed()) owner.sendGlobalEvent(name, value);
};
const facade = (kind) => ({
  store: native.store,
  host: {
    ...native.host,
    realqa: (raw, reply) => void realqa.invoke(raw, kind).then(reply),
  },
});
function showComposer() {
  if (composer && !composer.isDestroyed()) {
    composer.show();
    composer.focus();
    return;
  }
  const owner = new LynxWindow({
    width: 1200,
    height: 840,
    minWidth: 390,
    minHeight: 480,
    title: "RealQA",
    resizable: true,
  });
  composer = owner;
  owner.setAlwaysOnTop(true, "floating");
  const session = new WindowSession();
  owner.on("-lynx-invoke", (event, channel, request) => {
    if (channel !== "lane.native") return;
    const response = stopping
      ? Promise.resolve(unavailable)
      : session.invoke(facade("composer"), request);
    void response.then(
      (result) => event.sendReply(result),
      () => event.sendReply(unavailable),
    );
  });
  owner.on("close", (event) => {
    if (!quitting && realqa?.draft) {
      event.preventDefault();
      owner.sendGlobalEvent("lane.realqa.closeRequested", {});
    }
  });
  owner.on("closed", () => {
    session.retire();
    if (composer === owner) composer = undefined;
  });
  owner.once("ready-to-show", () =>
    owner.sendGlobalEvent("lane.realqa.composer", {}),
  );
  if (!owner.loadFile(join(resources, "main.lynx.bundle")))
    throw new Error("RealQA template could not load.");
  owner.show();
}

let started = false;
let quitting = false,
  stopping = false;
const pendingURLs = [];
const pendingRoutes = [];
const log = (event) =>
  console.info(JSON.stringify({ event, platform: process.platform }));
const failed = () => log("lane_host_operation_failed");
const runControl = (method, value) =>
  control(native, method, value).catch(failed);

function showWindow() {
  if (window && !window.isDestroyed()) {
    if (window.isMinimized()) window.restore();
    window.show();
    window.focus();
    return;
  }
  const session = new WindowSession();
  windowSession = session;
  window = new LynxWindow({
    width: 1280,
    height: 820,
    title: "Lane",
    resizable: true,
    ...(process.platform === "win32"
      ? { icon: join(resources, "Lane.ico") }
      : {}),
  });
  const owner = window;
  owner.on("-lynx-invoke", (event, channel, request) => {
    if (channel !== "lane.native") return;
    const response = stopping
      ? Promise.resolve(unavailable)
      : session.invoke(facade("main"), request);
    void response.then(
      (result) => event.sendReply(result),
      () => event.sendReply(unavailable),
    );
  });
  window.on("focus", () => runControl("resume"));
  window.on("closed", () => {
    session.retire();
    if (window === owner) {
      window = undefined;
      windowSession = undefined;
      runControl("viewClosed");
    }
  });
  window.on("--lynx-error", () => log("lane_template_failed"));
  window.once("ready-to-show", () => log("lane_template_loaded"));
  if (!window.loadFile(join(resources, "main.lynx.bundle")))
    throw new Error("The embedded Lane template could not be loaded.");
  window.show();
}
function activateURL(url) {
  // The native host owns complete URI, state and PKCE validation. Do not log it.
  if (typeof url !== "string" || !url.startsWith(`${scheme}://`)) return;
  if (!started) {
    pendingURLs.push(url);
    return;
  }
  showWindow();
  runControl("url", url);
}
function flushEvents() {
  for (const event of JSON.parse(native.poll())) {
    if (event.name === "lane.realqa.shortcut") {
      void realqa
        .invoke(
          JSON.stringify({ op: "capture", mode: event.arguments[0].mode }),
          "main",
        )
        .then((raw) => {
          const reply = JSON.parse(raw);
          if (!reply.ok) {
            showWindow();
            emitRealQA("lane.realqa.error", reply.error);
          }
        });
      continue;
    }
    if (event.name === "lane.activate") {
      showWindow();
      continue;
    }
    if (event.name === "lane.openRoute") pendingRoutes.push(event.arguments);
    else {
      for (const owner of [window, composer])
        if (owner && !owner.isDestroyed())
          owner.sendGlobalEvent(event.name, ...event.arguments);
    }
  }
  if (windowSession?.ready && window && !window.isDestroyed()) {
    for (const [name, value] of pendingRealQA.splice(0))
      window.sendGlobalEvent(name, value);
  }
  if (windowSession?.ready && window && !window.isDestroyed())
    for (const args of pendingRoutes.splice(0))
      window.sendGlobalEvent("lane.openRoute", ...args);
}

app.setName("Lane");
// Keep the Lynxtron singleton lock in a Lane-owned directory, separate from
// registry.sqlite and the shared CLI data. No renderer gets filesystem access.
app.setPath(
  "userData",
  native.testing && process.env.LANE_NATIVE_TEST_ROOT
    ? join(process.env.LANE_NATIVE_TEST_ROOT, "host")
    : join(app.getPath("appData"), "io.delino.lane", "host"),
);
app.on("open-url", (event, url) => {
  event.preventDefault();
  activateURL(url);
});
app.on("second-instance", (_event, args, _directory, data) => {
  if (started) showWindow();
  if (typeof data?.notificationRoute === "string" && data.notificationRoute) {
    try {
      pendingRoutes.push([JSON.parse(data.notificationRoute)]);
    } catch {
      failed();
    }
  }
  for (const value of args) activateURL(value);
});
app.on("activate", () => {
  if (started) {
    showWindow();
    runControl("resume");
  }
});
app.on("window-all-closed", () => {}); // The native synchronization service outlives views.
app.on("before-quit", (event) => {
  if (!started || quitting) return;
  event.preventDefault();
  if (stopping) return;
  stopping = true;
  // Keep event pumping active while native workers finish their UI callbacks.
  Promise.resolve(realqa?.stop())
    .then(() => control(native, "stop"))
    .then(() => {
      clearInterval(poller);
      tray?.destroy();
      quitting = true;
      realqa?.originals.clear();
      composer?.destroy();
      log("lane_stopped");
      app.quit();
    })
    .catch(() => {
      stopping = false;
      failed();
    });
});
const notificationRoute =
  process.platform === "win32" &&
  process.argv.includes("----AppNotificationActivated:")
    ? native.activation(resources)
    : "";
if (!app.requestSingleInstanceLock({ notificationRoute })) {
  app.quit();
} else {
  app
    .whenReady()
    .then(async () => {
      native.configure(resources, scheme);
      const dataRoot =
        native.testing && process.env.LANE_NATIVE_TEST_ROOT
          ? process.env.LANE_NATIVE_TEST_ROOT
          : process.platform === "darwin"
            ? join(homedir(), "Library/Application Support/io.delino.lane")
            : join(process.env.LOCALAPPDATA, "io.delino.lane");
      realqa = new RealQA({
        native,
        settingsPath: join(dataRoot, "realqa-shortcuts.json"),
        emit: emitRealQA,
        showComposer,
        closeComposer: () => composer?.destroy(),
        onSaved: (taskId, spaceId) => {
          showWindow();
          emitRealQA("lane.realqa.saved", { taskId, spaceId });
        },
        selectSource: (mode) => {
          showWindow();
          emitRealQA("lane.realqa.select", { mode });
        },
        beforeCapture: async () => {
          captureWindows = [window, composer].filter(
            (owner) => owner && !owner.isDestroyed() && owner.isVisible(),
          );
          for (const owner of captureWindows) owner.hide();
          await new Promise((resolve) => setTimeout(resolve, 150));
        },
        afterCapture: () => {
          if (stopping) return;
          for (const owner of captureWindows)
            if (!owner.isDestroyed()) owner.show();
          captureWindows = [];
          if (realqa.draft && composer && !composer.isDestroyed())
            composer.focus();
        },
      });
      await realqa.start();
      if (chromeExtensionIds.length) {
        try {
          realqa.extension = await connectChrome({
            realqa,
            dataRoot,
            extensionIds: chromeExtensionIds,
            showWindow,
          });
        } catch {
          log("lane_chrome_connection_unavailable");
        }
      }
      started = true;
      if (notificationRoute)
        pendingRoutes.push([JSON.parse(notificationRoute)]);
      if (native.testing)
        lynxBridge.handle("lane.testLog", (_event, request) => {
          if (
            typeof request?.message === "string" &&
            request.message.startsWith("LANE_NATIVE_")
          )
            console.log(request.message);
          return true;
        });
      if (!app.setAsDefaultProtocolClient(scheme))
        log("lane_protocol_registration_unavailable");
      Menu.setApplicationMenu(
        Menu.buildFromTemplate([
          {
            label: "Lane",
            submenu: [
              {
                label: "Open Lane",
                accelerator: "CmdOrCtrl+O",
                click: showWindow,
              },
              {
                label: "Quit Lane",
                accelerator: "CmdOrCtrl+Q",
                click: () => app.quit(),
              },
            ],
          },
          { role: "editMenu" },
        ]),
      );
      if (process.platform === "win32") {
        tray = new Tray(
          nativeImage.createFromPath(join(resources, "Lane.ico")),
        );
        tray.setToolTip("Lane");
        tray.setContextMenu(
          Menu.buildFromTemplate([
            { label: "Open Lane", click: showWindow },
            { label: "Quit Lane", click: () => app.quit() },
          ]),
        );
        tray.on("click", showWindow);
      }
      poller = setInterval(() => {
        try {
          flushEvents();
        } catch {
          failed();
        }
      }, 100);
      powerMonitor.on("resume", () => runControl("resume"));
      showWindow();
      for (const url of [...pendingURLs.splice(0), ...process.argv])
        activateURL(url);
      log("lane_started");
      if (
        native.testing &&
        process.env.LANE_NATIVE_TEST_ROOT &&
        process.env.LANE_NATIVE_TEST_CAPTURE === "1"
      ) {
        try {
          await realQASmoke(realqa);
        } catch {
          console.log("LANE_NATIVE_CAPTURE_FAILED");
        }
      }
      if (native.testing && process.env.LANE_NATIVE_TEST_LIFECYCLE === "1") {
        setTimeout(() => {
          window?.close();
          console.log("LANE_NATIVE_WINDOW_CLOSED");
          setTimeout(() => {
            showWindow();
            console.log("LANE_NATIVE_WINDOW_REOPENED");
            if (process.env.LANE_NATIVE_TEST_AUTOQUIT === "1")
              setTimeout(() => app.quit(), 2000);
          }, 2000);
        }, 2000);
      }
    })
    .catch(() => {
      log("lane_start_failed");
      app.quit();
    });
}
