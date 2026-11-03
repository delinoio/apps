// SPDX-License-Identifier: Apache-2.0
// The web-core runtime is copied beside this bundle at build time.
import "./runtime/static/js/client.js";
import LogtoClient from "@logto/browser";
import { installWebSupport } from "@packages/lynx-ui/web";
import { installAccessibility } from "./accessibility.js";
import { BrowserHost, CALLBACK_PATH } from "./browser-host.ts";

const meta = (name: string) =>
  document.querySelector<HTMLMetaElement>(`meta[name="${name}"]`)?.content ??
  "";
const savedPathKey = "anydash.sign-in-path";
const config = {
  api: meta("anydash-api") || "https://anydash.api.delino.io",
  endpoint: meta("anydash-logto-endpoint"),
  appId: meta("anydash-logto-app-id"),
  anydashResource: meta("anydash-resource") || "https://anydash.api.delino.io",
  delcoreResource: meta("delcore-resource") || "https://delcore.api.delino.io",
  version: meta("anydash-version"),
};
// Logto PKCE with two API resources: AnyDash for API calls, Delcore for
// ConnectAccount and ListSpaces. Tokens stay on this page, never in the worker.
const auth =
  config.endpoint && config.appId
    ? new LogtoClient({
        endpoint: config.endpoint,
        appId: config.appId,
        resources: [config.anydashResource, config.delcoreResource],
      })
    : undefined;
if (!auth)
  console.error(JSON.stringify({ event: "anydash_logto_unconfigured" }));

if (location.pathname === CALLBACK_PATH) {
  let target = "/";
  try {
    await auth?.handleSignInCallback(location.href);
    target = sessionStorage.getItem(savedPathKey) || "/";
  } catch {
    console.error(JSON.stringify({ event: "anydash_sign_in_callback_failed" }));
  }
  try {
    sessionStorage.removeItem(savedPathKey);
  } catch {
    /* Storage can be disabled; sign-in still completes. */
  }
  history.replaceState(
    {},
    "",
    target.startsWith("/") && !target.startsWith("//") ? target : "/",
  );
}

const digest = async (text: string) => {
  const bytes = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(text),
  );
  return Array.from(new Uint8Array(bytes), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
};
const host = new BrowserHost({
  config,
  auth,
  fetch: (...args) => fetch(...args),
  location,
  history,
  clipboard: navigator.clipboard,
  open: (url) => {
    window.open(url, "_blank", "noopener,noreferrer");
  },
  digest,
  savePath: (path) => {
    try {
      sessionStorage.setItem(savedPathKey, path);
    } catch {
      /* The destination falls back to the home page. */
    }
  },
});

await customElements.whenDefined("lynx-view");
type LynxView = HTMLElement & {
  globalProps: unknown;
  nativeModulesMap: Record<string, string>;
  onNativeModulesCall: (
    name: string,
    data: { method: string; payload: string },
    moduleName: string,
  ) => Promise<string>;
  sendGlobalEvent(name: string, params: unknown[]): void;
};
const view = document.createElement("lynx-view") as LynxView;
view.globalProps = {
  initialPath: `${location.pathname}${location.search}`,
  theme: matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light",
};
view.nativeModulesMap = {
  AnyDashHost: new URL("/host-module.js", location.href).href,
};
view.onNativeModulesCall = async (name, data, moduleName) =>
  moduleName === "AnyDashHost" && name === "call"
    ? host.reply(data.method, data.payload)
    : JSON.stringify({
        ok: false,
        error: { message: "Unsupported host module." },
      });
view.addEventListener("error", () =>
  console.error(
    JSON.stringify({ event: "anydash_template_failed", platform: "web" }),
  ),
);
document.body.append(view);
document.getElementById("boot")?.setAttribute("hidden", "");
const root = view.shadowRoot ?? view.attachShadow({ mode: "open" });
const disposeSupport = installWebSupport(root);
const disposeAccessibility = installAccessibility(view);
window.addEventListener("popstate", () =>
  view.sendGlobalEvent("anydash.route", [
    { path: `${location.pathname}${location.search}` },
  ]),
);
document.addEventListener("visibilitychange", () =>
  view.sendGlobalEvent("anydash.visibility", [{ visible: !document.hidden }]),
);
window.addEventListener("pagehide", (event) => {
  if (event.persisted) return;
  disposeSupport();
  disposeAccessibility();
});
view.setAttribute("url", "/main.web.bundle");
