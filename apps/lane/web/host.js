// SPDX-License-Identifier: Apache-2.0
import "./runtime/static/js/client.js";
import {
  focusAccessibleTarget,
  installAccessibility,
} from "./accessibility.js";
import {
  AuthRequestError,
  OnlineLane,
  createRouteDelivery,
  loginRoute,
} from "./online.js";
const api =
  document.querySelector('meta[name="lane-api"]')?.content ||
  "https://lane.api.delino.io";
let previousAccount;
let currentAccount;
let approvalPopup;
let workspaceDirty = false;
window.addEventListener("beforeunload", (event) => {
  if (workspaceDirty) {
    event.preventDefault();
    event.returnValue = "";
  }
});
const deliverRoute = createRouteDelivery();
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
const heading = document.getElementById("gate-heading");
const status = document.getElementById("account-status");
const guidance = document.getElementById("account-guidance");
const signIn = document.getElementById("sign-in");
const retry = document.getElementById("retry");
let lastViewFocus;
let startPending = false;
let presentationSeq = 0;
let signedOutTransitions = 0;
const GateState = Object.freeze({
  Connecting: "connecting",
  SignedOut: "signed-out",
  Error: "error",
});
const connectionGuidance = (offline, retained) =>
  offline
    ? retained
      ? "Your current-screen input is retained. Reconnect, then select Retry connection."
      : "Reconnect, then select Retry connection."
    : retained
      ? "Your current-screen input is retained. Retry connection explicitly."
      : "Retry connection explicitly.";
const activeViewElement = () => {
  let active = document.activeElement;
  while (active?.shadowRoot?.activeElement)
    active = active.shadowRoot.activeElement;
  return active;
};
const isVisible = (node) => {
  for (
    let current = node;
    current && current !== view;
    current = current.parentElement
  ) {
    if (
      current.hidden ||
      current.getAttribute("aria-hidden") === "true" ||
      getComputedStyle(current).display === "none" ||
      getComputedStyle(current).visibility === "hidden"
    )
      return false;
  }
  return !!node;
};
const activeViewHeading = () => {
  const panel = view.shadowRoot?.querySelector(
    '.tab-panel[data-active-tab="true"]',
  );
  const heading = panel?.querySelector('[role="heading"]');
  return isVisible(heading) ? heading : undefined;
};
const setGateState = (state, message, detail = "") => {
  gate.dataset.state = state;
  gate.setAttribute("aria-busy", String(state === GateState.Connecting));
  heading.textContent =
    state === GateState.Connecting
      ? "Connecting to Lane"
      : state === GateState.Error
        ? "Unable to connect"
        : "Sign in to Lane";
  status.textContent = message;
  guidance.textContent = detail;
  guidance.hidden = !detail;
  if (state === GateState.Connecting)
    retry.setAttribute("aria-disabled", "true");
  else retry.removeAttribute("aria-disabled");
};
const revealGate = () => {
  const viewWasFocused =
    document.activeElement === view || view.contains(document.activeElement);
  if (viewWasFocused) lastViewFocus = activeViewElement();
  gate.hidden = false;
  view.hidden = true;
  if (viewWasFocused) heading.focus();
};
const showSignedOut = () => {
  ++signedOutTransitions;
  ++presentationSeq;
  setGateState(GateState.SignedOut, "Sign in to use Lane on the web.");
  revealGate();
  lastViewFocus = undefined;
};
const showError = (error) => {
  const retained = !!currentAccount;
  const offline = !navigator.onLine;
  setGateState(
    GateState.Error,
    error instanceof Error
      ? error.message
      : "Connection failed. Retry explicitly.",
    retained || offline ? connectionGuidance(offline, retained) : "",
  );
  revealGate();
};
const restoreViewFocus = () => {
  const gateWasFocused = gate.contains(document.activeElement);
  gate.hidden = true;
  view.hidden = false;
  if (!gateWasFocused) {
    lastViewFocus = undefined;
    return;
  }
  const target =
    (lastViewFocus?.isConnected && lastViewFocus) ||
    activeViewHeading() ||
    view;
  if (target === view) view.tabIndex = -1;
  const normalizedTarget =
    target?.shadowRoot?.querySelector("input,textarea,#inner-box") || target;
  focusAccessibleTarget(target);
  if (target !== view && activeViewElement() !== normalizedTarget) {
    view.tabIndex = -1;
    focusAccessibleTarget(view);
  }
  lastViewFocus = undefined;
};
const online = new OnlineLane({
  baseUrl: api,
  returnTo,
  invalidate: () => view.sendGlobalEvent("lane.onlineChanged", [{}]),
  sessionChanged: (session) => {
    const changed =
      !!session &&
      previousAccount !== undefined &&
      previousAccount !== session.account_id;
    if (session) {
      if (!view.hasAttribute("url"))
        view.setAttribute("url", "./main.web.bundle");
      else if (!previousAccount || changed) view.reload();
    }
    previousAccount = session?.account_id;
    currentAccount = session?.account_id;
    if (session) {
      if (changed) lastViewFocus = undefined;
      ++presentationSeq;
      restoreViewFocus();
    } else {
      workspaceDirty = false;
      view.sendGlobalEvent("lane.sessionEnded", [{}]);
      showSignedOut();
    }
  },
});
view.nativeModulesMap = {
  LaneOnline: new URL("./online-module.js", location.href).href,
  LaneHost: new URL("./host-module.js", location.href).href,
};
view.onNativeModulesCall = async (name, data, moduleName) => {
  try {
    if (moduleName === "LaneOnline" && name === "ready") {
      const event = deliverRoute(location.search, currentAccount);
      if (event) view.sendGlobalEvent(event.name, [event.payload]);
      return { ok: true };
    }
    if (moduleName === "LaneOnline")
      return {
        ok: true,
        response: await online.invoke(
          data.service,
          data.method,
          JSON.parse(data.request),
        ),
      };
    if (moduleName === "LaneHost" && name === "workspace") {
      const value = JSON.parse(data.value);
      workspaceDirty = value.dirty === true || value.busy === true;
      return { ok: true };
    }
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
      approvalPopup = popup;
      if (!popup)
        throw new Error("Allow popups for Lane, then retry this action.");
      return { ok: true };
    }
    throw new Error("Unsupported host action.");
  } catch (e) {
    return {
      ok: false,
      error: {
        reason:
          e?.laneReason ||
          (e?.code === 7
            ? "PERMISSION_DENIED"
            : e?.code === 16
              ? "UNAUTHENTICATED"
              : "OPERATION_FAILED"),
        message: e instanceof Error ? e.message : "Operation failed.",
        violation: e?.laneViolation,
      },
    };
  }
};
view.addEventListener("error", () => {
  console.error({ event: "lane_template_failed", platform: "web" });
});
document.body.append(view);
installAccessibility(view);

view.hidden = true;
// A transient failure hides the existing view. Only a rejected session resets it.
const start = async () => {
  if (startPending) return;
  startPending = true;
  const attempt = ++presentationSeq;
  setGateState(GateState.Connecting, "Connecting…");
  try {
    await online.start();
  } catch (e) {
    if (
      attempt === presentationSeq &&
      !(e instanceof AuthRequestError && e.status === 401)
    )
      showError(e);
  } finally {
    startPending = false;
    gate.setAttribute("aria-busy", "false");
    retry.removeAttribute("aria-disabled");
  }
};
// Focus and online resume reconcile the API session before business requests
// continue, because an approval popup may have replaced the session cookie.
const resume = async () => {
  if (startPending || !gate.hidden || !navigator.onLine) return;
  const attempt = ++presentationSeq;
  try {
    await online.resume();
  } catch (e) {
    if (
      attempt === presentationSeq &&
      !(e instanceof AuthRequestError && e.status === 401)
    )
      showError(e);
  }
};
// Preserve the page destination across the full-page sign-in redirect.
signIn.href = online.loginUrl(loginRoute(location.search));
retry.addEventListener("click", start);
window.addEventListener("message", (event) => {
  if (
    event.origin !== location.origin ||
    event.source !== approvalPopup ||
    event.data?.type !== "lane.reauthenticated"
  )
    return;
  const signedOutBefore = signedOutTransitions;
  void online.completeDeletion(event.data.challengeId).catch((e) => {
    // Deletion proof expiry can return 401 while /auth/session remains valid.
    // Suppress 401 only when this reconciliation actually showed the signed-out gate.
    const reconciledToSignedOut = signedOutTransitions !== signedOutBefore;
    if (!(
      e instanceof AuthRequestError &&
      e.status === 401 &&
      reconciledToSignedOut
    ))
      showError(e);
  });
});
window.addEventListener("online", () => {
  if (!gate.hidden && gate.dataset.state !== GateState.Connecting) {
    guidance.textContent = connectionGuidance(false, !!currentAccount);
    guidance.hidden = false;
  }
  void resume();
});
window.addEventListener("offline", () => {
  view.sendGlobalEvent("lane.onlineChanged", [{}]);
  if (!gate.hidden && gate.dataset.state !== GateState.Connecting) {
    guidance.textContent = connectionGuidance(true, !!currentAccount);
    guidance.hidden = false;
  }
});
window.addEventListener("focus", () => void resume());
window.addEventListener("pagehide", () => online.stop());
await start();
