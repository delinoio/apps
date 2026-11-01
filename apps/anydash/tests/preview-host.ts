// SPDX-License-Identifier: Apache-2.0
// Browser preview host: the production web bundle against the in-memory API.
// Used by tests/browser.mjs only; never deployed.
import "../dist/runtime/static/js/client.js";
import { installWebSupport } from "@packages/lynx-ui/web";
import {
  ErrorReason,
  EnrollmentState,
  OnboardingState,
} from "@repo/protos/anydash/v1/anydash_pb";
import { installAccessibility } from "../web/accessibility.js";
import { HostPlatform } from "../src/host";
import { FakeHost, type FakeHostOptions } from "./fixtures/fake-host";

export const scenarios: Record<string, FakeHostOptions> = {
  states: {},
  team: { team: true },
  onboarding: { onboarding: OnboardingState.DEVICE_REQUIRED, team: true },
  empty: { personalDashboards: false },
  loading: { pendingData: true },
  denied: { dashboardError: ErrorReason.PERMISSION_DENIED },
  error: { dashboardError: ErrorReason.INTERNAL },
  conflict: { conflictOnLayout: true },
  update: { updateRequired: true },
  approval: {},
  invalid: { enrollmentState: EnrollmentState.INVALIDATED },
  desktop: { platform: HostPlatform.Desktop },
  signedOut: { signedIn: false },
};
const key = "anydash.preview.scenario";
const fromUrl = new URLSearchParams(location.search).get("scenario");
if (fromUrl) sessionStorage.setItem(key, fromUrl);
const name = sessionStorage.getItem(key) || "states";
const host = new FakeHost(scenarios[name] ?? {});
await customElements.whenDefined("lynx-view");
type View = HTMLElement & {
  globalProps: unknown;
  nativeModulesMap: Record<string, string>;
  onNativeModulesCall: (
    n: string,
    d: { method: string; payload: string },
    m: string,
  ) => Promise<string>;
  sendGlobalEvent(name: string, params: unknown[]): void;
};
const view = document.createElement("lynx-view") as View;
const initial = new URL(location.href);
initial.searchParams.delete("scenario");
view.globalProps = {
  initialPath: `${initial.pathname}${initial.search}`,
  theme: "light",
};
view.nativeModulesMap = {
  AnyDashHost: new URL("/host-module.js", location.href).href,
};
view.onNativeModulesCall = (_name, data) =>
  new Promise((resolve) => {
    if (data.method === "route") {
      const { path, replace } = JSON.parse(data.payload);
      if (replace) history.replaceState({}, "", path);
      else history.pushState({}, "", path);
    }
    host.call(data.method, data.payload, (raw) => resolve(String(raw)));
  });
document.body.append(view);
document.getElementById("boot")?.setAttribute("hidden", "");
installWebSupport(view.shadowRoot ?? view.attachShadow({ mode: "open" }));
installAccessibility(view);
window.addEventListener("popstate", () =>
  view.sendGlobalEvent("anydash.route", [
    { path: `${location.pathname}${location.search}` },
  ]),
);
(window as unknown as { anydashPreview: unknown }).anydashPreview = {
  host,
  name,
};
view.setAttribute("url", "/main.web.bundle");
