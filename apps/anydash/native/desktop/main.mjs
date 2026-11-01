// SPDX-License-Identifier: Apache-2.0
// AnyDash desktop main process (Lynxtron). It stays resident in the macOS menu
// bar or the Windows notification area after the window closes, so the
// collector it started keeps collecting and native widget snapshots stay
// current. Quit stops the app and the collector it started.
import {
  app,
  clipboard,
  LynxWindow,
  Menu,
  nativeImage,
  shell,
  Tray,
} from "@lynx-js/lynxtron";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { NativeAuth } from "./auth.mjs";
import { Collector } from "./collector.mjs";
import { DesktopHost } from "./host.mjs";
import { createSecureStore, memoryStore } from "./secure-store.mjs";
import { DesktopUpdater } from "./updater.mjs";

const resources = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const config = require("./package.json");
const scheme = "anydash";
const log = (event) =>
  console.info(JSON.stringify({ platform: process.platform, ...event }));
let window, tray, host, auth, collector, updater;
let started = false;
let quitting = false;
let stopping = false;
/** Deep links received before the app finished starting. */
const pendingURLs = [];
let lastPath = "/";

app.setName("AnyDash");
app.setPath(
  "userData",
  join(app.getPath("appData"), "io.delino.anydash", "app"),
);

const send = (name, value) => {
  if (window && !window.isDestroyed()) window.sendGlobalEvent(name, value);
};

function showWindow() {
  if (window && !window.isDestroyed()) {
    if (window.isMinimized()) window.restore();
    window.show();
    window.focus();
    return;
  }
  window = new LynxWindow({
    width: 1320,
    height: 860,
    minWidth: 390,
    minHeight: 480,
    title: "AnyDash",
    resizable: true,
    ...(process.platform === "win32"
      ? { icon: join(resources, "AnyDash.ico") }
      : {}),
  });
  const owner = window;
  owner.on("-lynx-invoke", (event, channel, request) => {
    if (channel !== "anydash.host" || stopping) {
      event.sendReply(
        JSON.stringify({
          ok: false,
          error: { message: "AnyDash is quitting." },
        }),
      );
      return;
    }
    void host
      .reply(String(request?.method ?? ""), String(request?.payload ?? ""))
      .then((reply) => event.sendReply(reply));
  });
  owner.on("focus", () => send("anydash.visibility", { visible: true }));
  owner.on("minimize", () => send("anydash.visibility", { visible: false }));
  owner.on("restore", () => send("anydash.visibility", { visible: true }));
  owner.on("closed", () => {
    if (window === owner) window = undefined;
    log({ event: "anydash_window_closed", resident: true });
  });
  if (
    !owner.loadFile(join(resources, "main.lynx.bundle"), {
      globalProps: { initialPath: lastPath },
    })
  )
    throw new Error("The embedded AnyDash template could not be loaded.");
  owner.show();
}

/** Deep links: auth callbacks complete sign-in; others open their route. */
function activateURL(url) {
  if (typeof url !== "string" || !url.startsWith(`${scheme}://`)) return;
  if (!started) {
    pendingURLs.push(url);
    return;
  }
  if (url.startsWith(`${scheme}://auth/callback`)) {
    void auth.callback(url).then((handled) => {
      if (handled) {
        showWindow();
        send("anydash.session", {});
      }
    });
    return;
  }
  // The shared client validates every route; unknown links show "not found".
  const wasOpen = !!window && !window.isDestroyed();
  lastPath = url;
  showWindow();
  if (wasOpen) send("anydash.route", { path: url });
}

app.on("open-url", (event, url) => {
  event.preventDefault();
  activateURL(url);
});
app.on("second-instance", (_event, args) => {
  if (started) showWindow();
  for (const value of args) activateURL(value);
});
app.on("activate", () => {
  if (started) showWindow();
});
// Closing the window keeps the app and its collector running.
app.on("window-all-closed", () => {});
app.on("before-quit", (event) => {
  if (!started || quitting) return;
  event.preventDefault();
  if (stopping) return;
  stopping = true;
  void collector
    .quit()
    .then((result) => log({ event: "anydash_collector_quit", ...result }))
    .finally(() => {
      tray?.destroy();
      quitting = true;
      app.quit();
    });
});

if (!app.requestSingleInstanceLock()) app.quit();
else
  app
    .whenReady()
    .then(async () => {
      const testing = config.testing === true;
      collector = new Collector({ resources, log });
      auth = new NativeAuth({
        endpoint: config.logtoEndpoint,
        appId: config.logtoAppId,
        resources: [config.anydashResource, config.delcoreResource],
        store: testing ? memoryStore() : createSecureStore(),
        openBrowser: (url) => shell.openExternal(url),
      });
      updater = new DesktopUpdater({
        config,
        install: resolve(
          resources,
          process.platform === "darwin" ? "../../.." : "../..",
        ),
        stateDirectory: app.getPath("userData"),
        beforeInstall: async () => {
          stopping = true;
          await collector.quit();
        },
        relaunch: () => {
          quitting = true;
          app.quit();
        },
        log,
      });
      void updater.cleanup();
      host = new DesktopHost({
        config,
        auth,
        collector,
        clipboard,
        openExternal: (url) => shell.openExternal(url),
        loginItems: {
          // Starting at login is opt-in; the app then starts hidden in the tray.
          get: () => app.getLoginItemSettings().openAtLogin,
          set: (enabled) =>
            app.setLoginItemSettings({
              openAtLogin: enabled,
              openAsHidden: true,
            }),
        },
        updater,
        cacheDirectory: join(app.getPath("userData"), "cache"),
        platformLabel: `${process.platform === "darwin" ? "macOS" : "Windows"} ${process.arch}`,
        onRoute: (path) => {
          lastPath = path;
        },
      });
      if (!(await collector.ensure()))
        log({ event: "anydash_collector_unavailable" });
      started = true;
      if (!app.setAsDefaultProtocolClient(scheme))
        log({ event: "anydash_protocol_registration_unavailable" });
      const menu = Menu.buildFromTemplate([
        { label: "Open AnyDash", click: showWindow },
        { type: "separator" },
        { label: "Quit AnyDash", click: () => app.quit() },
      ]);
      tray = new Tray(
        nativeImage.createFromPath(
          join(
            resources,
            process.platform === "darwin" ? "TrayTemplate.png" : "AnyDash.ico",
          ),
        ),
      );
      tray.setToolTip("AnyDash");
      tray.setContextMenu(menu);
      tray.on("click", showWindow);
      Menu.setApplicationMenu(
        Menu.buildFromTemplate([
          {
            label: "AnyDash",
            submenu: [
              {
                label: "Open AnyDash",
                accelerator: "CmdOrCtrl+O",
                click: showWindow,
              },
              {
                label: "Quit AnyDash",
                accelerator: "CmdOrCtrl+Q",
                click: () => app.quit(),
              },
            ],
          },
          { role: "editMenu" },
        ]),
      );
      const hidden = app.getLoginItemSettings().wasOpenedAsHidden;
      if (!hidden) showWindow();
      for (const url of [...pendingURLs.splice(0), ...process.argv])
        activateURL(url);
      log({ event: "anydash_started", collectorStarted: collector.started });
    })
    .catch(() => {
      log({ event: "anydash_start_failed" });
      app.quit();
    });
