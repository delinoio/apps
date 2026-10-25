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
import { control, invokeNative, unavailable } from "./bridge.mjs";

const resources = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const { scheme } = require("./package.json");
const native = require("./lane_host.node");
let window, tray, poller;
let started = false;
let ready = false,
  quitting = false,
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
  ready = false;
  window = new LynxWindow({
    width: 1280,
    height: 820,
    title: "Lane",
    resizable: true,
  });
  window.on("focus", () => runControl("resume"));
  window.on("closed", () => {
    ready = false;
    window = undefined;
    runControl("viewClosed");
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
    if (event.name === "lane.activate") {
      showWindow();
      continue;
    }
    if (event.name === "lane.openRoute") pendingRoutes.push(event.arguments);
    else if (window && !window.isDestroyed())
      window.sendGlobalEvent(event.name, ...event.arguments);
  }
  if (ready && window && !window.isDestroyed())
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
  control(native, "stop")
    .then(() => {
      clearInterval(poller);
      tray?.destroy();
      quitting = true;
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
    .then(() => {
      native.configure(resources, scheme);
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
      lynxBridge.handle("lane.native", async (_event, request) => {
        if (stopping) return unavailable;
        const result = await invokeNative(native, request);
        if (
          request?.module === "LaneStore" &&
          request.method === "open" &&
          JSON.parse(result).ok
        )
          ready = true;
        return result;
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
        // A small code-owned tray mark needs no extra binary asset.
        const pixels = Buffer.alloc(16 * 16 * 4);
        for (let y = 0; y < 16; y++)
          for (let x = 0; x < 16; x++) {
            const white =
              (x >= 4 && x <= 6 && y >= 3 && y <= 12) ||
              (y >= 10 && y <= 12 && x >= 4 && x <= 12);
            pixels.set(
              white ? [255, 255, 255, 255] : [235, 99, 37, 255],
              (y * 16 + x) * 4,
            );
          }
        tray = new Tray(
          nativeImage.createFromBitmap(pixels, { width: 16, height: 16 }),
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
