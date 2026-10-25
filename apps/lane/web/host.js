// SPDX-License-Identifier: Apache-2.0
import "./runtime/static/js/client.js";
import { installAccessibility } from "./accessibility.js";
import { OnlineLane } from "./online.js";
const api =
  document.querySelector('meta[name="lane-api"]')?.content ||
  "https://lane.api.delino.io";
const returnTo = new URL("/", location.href).href;
const callbackChallenge = new URL(location.href).searchParams.get(
  "reauth_challenge",
);
if (window.opener && callbackChallenge) {
  window.opener.postMessage(
    { type: "lane.reauthenticated", challengeId: callbackChallenge },
    location.origin,
  );
  window.close();
}
await customElements.whenDefined("lynx-view");
const view = document.createElement("lynx-view");
const gate = document.getElementById("account-gate");
const status = document.getElementById("account-status");
const signIn = document.getElementById("sign-in");
const retry = document.getElementById("retry");
const online = new OnlineLane({
  baseUrl: api,
  returnTo,
  invalidate: () => view.sendGlobalEvent("lane.onlineChanged", [{}]),
  sessionChanged: (session) => {
    gate.hidden = !!session;
    view.hidden = !session;
    if (!session) {
      view.sendGlobalEvent("lane.sessionEnded", [{}]);
      status.textContent = "Sign in to use Lane on the web.";
    }
  },
});
view.nativeModulesMap = {
  LaneOnline: new URL("./online-module.js", location.href).href,
  LaneHost: new URL("./host-module.js", location.href).href,
};
view.onNativeModulesCall = async (name, data, moduleName) => {
  try {
    if (moduleName === "LaneOnline")
      return {
        ok: true,
        response: await online.invoke(
          data.service,
          data.method,
          JSON.parse(data.request),
        ),
      };
    if (moduleName === "LaneHost" && name === "copyText") {
      await navigator.clipboard.writeText(data.text);
      return { ok: true };
    }
    if (moduleName === "LaneHost" && name === "openURL") {
      const target = new URL(data.url);
      if (target.protocol !== "https:")
        throw new Error("Only HTTPS links can be opened.");
      const popup = window.open(
        target.href,
        "lane-approval",
        "popup,width=720,height=800",
      );
      if (!popup)
        throw new Error("Allow popups for Lane, then retry this action.");
      return { ok: true };
    }
    throw new Error("Unsupported host action.");
  } catch (e) {
    return {
      ok: false,
      error: {
        reason: "OPERATION_FAILED",
        message: e instanceof Error ? e.message : "Operation failed.",
      },
    };
  }
};
view.addEventListener("error", () => {
  console.error({ event: "lane_template_failed", platform: "web" });
});
document.body.append(view);
installAccessibility(view);
view.setAttribute("url", "./main.web.bundle");
view.hidden = true;
const start = async () => {
  status.textContent = "Connecting…";
  try {
    await online.start();
  } catch (e) {
    gate.hidden = false;
    status.textContent =
      e instanceof Error ? e.message : "Connection failed. Retry explicitly.";
  }
};
signIn.href = online.loginUrl();
retry.addEventListener("click", start);
window.addEventListener("message", (event) => {
  if (
    event.origin !== location.origin ||
    event.data?.type !== "lane.reauthenticated"
  )
    return;
  void online.completeDeletion(event.data.challengeId).catch((e) => {
    status.textContent = e.message;
    gate.hidden = false;
  });
});
window.addEventListener("online", () => online.resume());
window.addEventListener("offline", () =>
  view.sendGlobalEvent("lane.onlineChanged", [{}]),
);
window.addEventListener("focus", () => online.resume());
window.addEventListener("pagehide", () => online.stop());
await start();
