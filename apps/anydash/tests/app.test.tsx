// SPDX-License-Identifier: Apache-2.0
import { afterEach, describe, expect, it } from "@rstest/core";
import { act, cleanup, waitFor } from "@lynx-js/react/testing-library";
import { create, fromBinary, toBinary } from "@bufbuild/protobuf";
import { base64Decode, base64Encode } from "@bufbuild/protobuf/wire";
import {
  ErrorReason,
  NativeWidgetSnapshotSchema,
  OnboardingState,
  SharingState,
  SpacePermission,
  WidgetDataSchema,
} from "@repo/protos/anydash/v1/anydash_pb";
import { Copy, ExtraCopy } from "../src/copy";
import { HostPlatform } from "../src/host";
import { widgetKey } from "../src/model/cache";
import * as fixtures from "./fixtures/scenarios";
import { renderApp, type Rendered } from "./harness";

let app: Rendered | undefined;
afterEach(() => {
  app?.unmount();
  app = undefined;
  cleanup();
});
const dashboardPath = `/spaces/${fixtures.ids.personal}/dashboards/${fixtures.ids.overview}`;
const teamPath = `/spaces/${fixtures.ids.team}/dashboards/${fixtures.ids.teamBoard}`;

describe("onboarding", () => {
  it("blocks the personal space until a collector device is connected", async () => {
    app = await renderApp({
      onboarding: OnboardingState.DEVICE_REQUIRED,
      team: true,
    });
    await app.see(Copy.connectDevice);
    expect(app.text()).toContain(Copy.connectDeviceDetail);
    expect(app.text()).toContain(Copy.downloadApp);
    expect(app.text()).toContain(Copy.installCli);
    expect(app.text()).toContain(Copy.teamWithoutDevice);
    expect(
      app.host.api.calls.some(
        (call) =>
          call.method === "ListDashboards" &&
          (call.request as { spaceId: string }).spaceId ===
            fixtures.ids.personal,
      ),
    ).toBe(false);
    await app.tap(Copy.installCli);
    await app.see("curl -fsSL https://anydash.delino.io/install.sh | sh");
  });

  it("opens team dashboards without a personal device", async () => {
    app = await renderApp({
      onboarding: OnboardingState.DEVICE_REQUIRED,
      team: true,
      path: teamPath,
    });
    await app.see("Codex · 5-hour limit");
    await app.see("62% left");
    expect(app.text()).not.toContain(Copy.connectDevice);
  });

  it("enrolls this computer through the collector on desktop", async () => {
    app = await renderApp({
      onboarding: OnboardingState.DEVICE_REQUIRED,
      platform: HostPlatform.Desktop,
    });
    await app.see(ExtraCopy.connectThisComputer);
    expect(app.text()).not.toContain(Copy.downloadApp);
  });
});

describe("session", () => {
  it("leaves the Delcore token to the host", async () => {
    app = await renderApp({ team: true });
    await app.see("62% left");
    expect(app.host.calls.some((call) => call.method === "delcoreToken")).toBe(
      false,
    );
    const carriers = app.host.api.calls.filter(
      (call) =>
        call.method === "ConnectAccount" || call.method === "ListSpaces",
    );
    expect(carriers.map((call) => call.method)).toEqual([
      "ConnectAccount",
      "ListSpaces",
    ]);
    for (const call of carriers)
      expect(
        (call.request as { delcoreAccessToken: string }).delcoreAccessToken,
      ).toBe("");
  });
});

describe("dashboard states", () => {
  it("renders every metric state, the duplicate notice and the header", async () => {
    app = await renderApp({ path: dashboardPath });
    await app.see("62% left");
    for (const value of [
      "Not provided",
      "Couldn’t refresh",
      "Authentication expired",
      "Partial aggregation · 12 records skipped",
      "Can’t be calculated",
      "Sharing ended",
      "$41.80",
      Copy.duplicateDevices(3),
      Copy.loading,
      "Asia/Seoul · Updated",
      Copy.refresh,
      Copy.edit,
    ])
      expect(app.text()).toContain(value);
    const groups = app.find("[accessibility-role-description='group']");
    expect(groups.length).toBe(9);
    expect(groups[0].getAttribute("accessibility-label")).toBe(
      "Codex · 5-hour limit",
    );
  });

  it("hides Edit when the space does not grant it", async () => {
    app = await renderApp({
      team: true,
      teamPermissions: [SpacePermission.VIEW_DASHBOARDS],
      path: teamPath,
    });
    await app.see("62% left");
    expect(app.text()).not.toContain(Copy.edit);
  });

  it("shows the update-required banner and blocks server features", async () => {
    app = await renderApp({ updateRequired: true, path: dashboardPath });
    await app.see(Copy.updateRequiredDetail);
    expect(app.text()).not.toContain("62% left");
  });

  it("shows access denied for dashboards outside the caller's roles", async () => {
    app = await renderApp({
      dashboardError: ErrorReason.PERMISSION_DENIED,
      path: dashboardPath,
    });
    await app.see(Copy.accessDenied);
    expect(app.text()).toContain(Copy.accessDeniedDetail);
    expect(app.text()).toContain(Copy.goToDashboards);
  });

  it("shows access denied for dashboard links to spaces the caller left", async () => {
    const leftSpace = "0192f000-0000-7000-8000-0000000000b9";
    app = await renderApp({
      path: `/spaces/${leftSpace}/dashboards/${fixtures.ids.teamBoard}`,
    });
    await app.see(Copy.accessDenied);
    expect(app.text()).toContain(Copy.accessDeniedDetail);
    // The spaces are reloaded once in case the caller joined after sign-in.
    const listSpaces = () =>
      app!.host.api.calls.filter((call) => call.method === "ListSpaces").length;
    expect(listSpaces()).toBe(2);
    await app.tap(Copy.goToDashboards);
    await app.see("62% left");
    expect(app.text()).not.toContain(Copy.accessDenied);
  });

  it("shows the error screen with copyable correlation IDs", async () => {
    app = await renderApp({
      dashboardError: ErrorReason.INTERNAL,
      path: dashboardPath,
    });
    // Unexpected server failures retry twice before the error screen.
    await app.see(Copy.somethingWrong, 10_000);
    await app.see(`${Copy.correlationId} 0192f9aa-`);
    await app.tap(Copy.copyDetails);
    expect(app.host.copied.at(-1)).toMatch(/^Correlation ID 0192f9aa-/);
  }, 15_000);

  it("shows the loading skeleton while data is pending", async () => {
    app = await renderApp({ pendingData: true, path: dashboardPath });
    await app.see(Copy.loadingDashboard);
    expect(app.find(".ad-sk").length).toBeGreaterThan(0);
  });

  it("shows the empty state and the dashboard quota guidance", async () => {
    app = await renderApp({ personalDashboards: false, quotaOnCreate: true });
    await app.see(Copy.noDashboards);
    expect(app.text()).toContain(Copy.dashboardLimit);
    await app.tap(Copy.createDashboard);
    await app.see(ExtraCopy.dashboardName);
  });

  it("limits manual refresh to once per minute and shows the wait", async () => {
    app = await renderApp({ path: dashboardPath });
    await app.see("62% left");
    await app.tap(Copy.refresh);
    await app.see("Refresh available in");
    const refresh = app
      .find(".ad-small-button")
      .find((element) => element.textContent === Copy.refresh)!;
    expect(refresh.classList.contains("lui-disabled")).toBe(true);
    expect(refresh.getAttribute("accessibility-label")).toMatch(
      /^Refresh\. Refresh available in \d+s$/,
    );
    expect(
      app.host.api.calls.filter(
        (call) => call.method === "RequestConnectionRefresh",
      ).length,
    ).toBeGreaterThan(0);
  });
});

describe("layouts", () => {
  it.each([
    [1440, ".ad-cell"],
    [900, ".ad-flow-half"],
    [390, ".ad-flow-full"],
  ])("lays out widgets for width %i", async (width, selector) => {
    app = await renderApp({ path: dashboardPath, width });
    await app.see("62% left");
    expect(app.find(selector).length).toBe(9);
  });
});

describe("phone layout", () => {
  it("keeps the dashboard picker on phones", async () => {
    app = await renderApp({
      path: dashboardPath,
      width: 390,
      platform: HostPlatform.Ios,
    });
    await app.see("62% left");
    await app.tap(`${ExtraCopy.chooseDashboard}: Overview`);
    await app.see(ExtraCopy.chooseDashboard);
    await app.see("✓ Overview");
  });
});

describe("editor", () => {
  it("recovers from a version conflict by loading the latest dashboard", async () => {
    app = await renderApp({ path: dashboardPath, conflictOnLayout: true });
    await app.see("62% left");
    await app.tap(Copy.edit);
    await app.see(ExtraCopy.editorHint);
    const card = app.find(".ad-widget")[0];
    await app.tap(card.getAttribute("accessibility-label")!);
    await app.key(card, "ArrowRight");
    await app.see("column 2, row 1, 4 columns wide, 2 rows tall.");
    await app.key(card, "ArrowDown", true);
    await app.see("column 2, row 1, 4 columns wide, 3 rows tall.");
    await app.tap(Copy.done);
    await app.see(Copy.dashboardChanged);
    expect(app.text()).toContain(Copy.dashboardChangedDetail);
    const saves = () =>
      app!.host.api.calls.filter(
        (call) => call.method === "UpdateWidgetLayout",
      );
    expect(saves().length).toBe(1);
    await app.tap(Copy.continueEditing);
    await app.see(ExtraCopy.editorHint);
    // The draft restarts from the latest version; nothing is merged silently.
    const fresh = app.find(".ad-widget")[0];
    expect(fresh.getAttribute("accessibility-label")).toContain(
      "column 1, row 1",
    );
    await app.tap(fresh.getAttribute("accessibility-label")!);
    await app.tap(ExtraCopy.moveRight);
    await app.tap(Copy.done);
    await app.see("62% left");
    expect(saves().length).toBe(2);
    expect(
      (saves()[1].request as { expectedVersion: bigint }).expectedVersion,
    ).toBe(8n);
  });

  it("keeps unsaved moves when a widget is added or removed while editing", async () => {
    app = await renderApp({ path: dashboardPath });
    await app.see("62% left");
    await app.tap(Copy.edit);
    await app.see(ExtraCopy.editorHint);
    const card = app.find(".ad-widget")[0];
    const label = card.getAttribute("accessibility-label")!;
    await app.tap(label);
    await app.key(card, "ArrowRight");
    await app.see("column 2, row 1, 4 columns wide, 2 rows tall.");
    const position = () =>
      app!.find(".ad-widget")[0].getAttribute("accessibility-label")!;
    const calls = (method: string) =>
      app!.host.api.calls.filter((call) => call.method === method);
    await app.tap(ExtraCopy.addWidget);
    await app.tap("Codex · Codex", ".lui-dialog");
    await app.tap(ExtraCopy.addWidget, ".lui-dialog");
    await waitFor(() => expect(calls("AddWidget").length).toBe(1));
    await waitFor(() =>
      expect(app!.find(".ad-widget").length).toBe(
        fixtures.makeDashboard().widgets.length + 1,
      ),
    );
    expect(position()).toContain("column 2, row 1");
    const added = app.find(".ad-widget").at(-1)!;
    await app.tap(added.getAttribute("accessibility-label")!);
    await app.tap(Copy.removeWidget);
    await waitFor(() => expect(calls("RemoveWidget").length).toBe(1));
    await waitFor(() =>
      expect(app!.find(".ad-widget").length).toBe(
        fixtures.makeDashboard().widgets.length,
      ),
    );
    expect(position()).toContain("column 2, row 1");
    await app.tap(Copy.done);
    await waitFor(() => expect(calls("UpdateWidgetLayout").length).toBe(1));
    const save = calls("UpdateWidgetLayout")[0].request as {
      expectedVersion: bigint;
      updates: { widgetId: string; layout: { x: number; y: number } }[];
    };
    const moved = save.updates.find(
      (update) => update.widgetId === fixtures.ids.widget(1),
    );
    expect(moved?.layout.x).toBe(1);
  });

  it("confirms the shared scope before adding a widget to a team dashboard", async () => {
    app = await renderApp({ team: true, path: teamPath });
    await app.see("62% left");
    await app.tap(Copy.edit);
    await app.tap(ExtraCopy.addWidget);
    await app.tap("Codex · Codex", ".lui-dialog");
    await app.tap(ExtraCopy.addWidget, ".lui-dialog");
    await app.see(Copy.shareTitle("Team Acme"));
    expect(app.text()).toContain("Codex subscription limits");
    expect(app.text()).toContain(
      "They can’t see your connection, credentials or any other data.",
    );
    await app.key(app.find(".lui-dialog-backdrop")[0], "Escape");
    expect(app.text()).not.toContain(Copy.shareTitle("Team Acme"));
    expect(app.host.api.calls.some((call) => call.method === "AddWidget")).toBe(
      false,
    );
  });
});

describe("sharing ended", () => {
  it("purges the cached data and native snapshot of an ended widget", async () => {
    const ended = fixtures.ids.widget(1);
    const cached = fixtures
      .makeWidgetData()
      .find((entry) => entry.widgetId === ended)!;
    const cache = new Map([
      [
        widgetKey("user-key", ended),
        base64Encode(toBinary(WidgetDataSchema, cached)),
      ],
    ]);
    app = await renderApp({
      path: dashboardPath,
      cache,
      endedWidgetIds: [ended],
      platform: HostPlatform.Desktop,
    });
    await app.see("Couldn’t refresh");
    await app.see(Copy.sharingEndedDetail);
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(app.host.cache.has(widgetKey("user-key", ended))).toBe(false);
    expect(app.text()).not.toContain("62% left");
    const snapshot = app.host.api.snapshots.get(ended);
    expect(snapshot?.sharingState).toBe(SharingState.ENDED);
    expect(snapshot?.summary).toBeUndefined();
    // Other widgets keep their cached copy for offline launches.
    expect(
      app.host.cache.has(widgetKey("user-key", fixtures.ids.widget(2))),
    ).toBe(true);
    // The purge settles: stripping ended data must not re-trigger itself.
    const writes = () =>
      app!.host.api.calls.filter(
        (call) => call.method === "PutNativeWidgetSnapshots",
      ).length;
    const settled = writes();
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(writes()).toBe(settled);
    expect(settled).toBeLessThanOrEqual(2);
  });

  it("writes ENDED snapshots for mobile widget extensions", async () => {
    app = await renderApp({
      path: dashboardPath,
      endedWidgetIds: [fixtures.ids.widget(1)],
      platform: HostPlatform.Ios,
    });
    await app.see(Copy.sharingEndedDetail);
    await new Promise((resolve) => setTimeout(resolve, 50));
    const snapshots = app.host.mobileSnapshots.map((raw) =>
      fromBinary(NativeWidgetSnapshotSchema, base64Decode(raw)),
    );
    const ended = snapshots.find(
      (entry) => entry.widgetId === fixtures.ids.widget(1),
    );
    expect(ended?.sharingState).toBe(SharingState.ENDED);
    expect(ended?.summary).toBeUndefined();
    // Mobile apps are viewers: no editing controls.
    expect(app.text()).not.toContain(Copy.edit);
  });
});

describe("device approval", () => {
  const path = `/connect/${fixtures.ids.enrollment}`;
  it("shows the device, expiry and attempts, then approves the code", async () => {
    app = await renderApp({ path, codeMismatch: true });
    await app.see(Copy.approveDevice);
    expect(app.text()).toContain("studio-mac · macOS arm64 · Collector 1.0.3");
    expect(app.text()).toContain(Copy.enterCode);
    await app.see("attempts left");
    expect(app.text()).toMatch(/Expires in \d+:\d\d · 4 attempts left/);
    const input = app.find("input.ad-code-input")[0];
    expect(input.getAttribute("accessibility-label")).toBe(Copy.enterCode);
    // The single critical input receives focus on load.
    const ui =
      (globalThis as { __anydashUiCalls?: { method: string }[] })
        .__anydashUiCalls ?? [];
    expect(ui.some((call) => call.method === "focus")).toBe(true);
    const type = async (value: string) => {
      await act(async () => {
        input.dispatchEvent(
          new CustomEvent("bindEvent:input", { detail: { value } }),
        );
      });
    };
    await type("bcdf-ghjk");
    await app.tap(Copy.approve);
    await app.see(ExtraCopy.codeMismatch);
    await app.see("3 attempts left");
    await app.tap(Copy.approve);
    await app.see(ExtraCopy.deviceApproved);
  });

  it("reloads spaces after approving the first device", async () => {
    app = await renderApp({
      path,
      onboarding: OnboardingState.DEVICE_REQUIRED,
    });
    await app.see(Copy.approveDevice);
    const input = app.find("input.ad-code-input")[0];
    await act(async () => {
      input.dispatchEvent(
        new CustomEvent("bindEvent:input", { detail: { value: "bcdf-ghjk" } }),
      );
    });
    await app.tap(Copy.approve);
    await app.see(ExtraCopy.deviceApproved);
    expect(
      app.host.api.calls.filter((call) => call.method === "ListSpaces").length,
    ).toBe(2);
    await app.tap(Copy.goToDashboards);
    await app.see(Copy.noDashboards);
    expect(app.text()).not.toContain(Copy.connectDevice);
  });

  it("explains invalid codes", async () => {
    app = await renderApp({ path, enrollmentState: 6 });
    await app.see(Copy.codeInvalid);
    expect(app.text()).toContain(
      "The code expired, was already used, or was entered wrong 5 times. Run anydash login again, or restart sign-in in the app, to get a new code.",
    );
  });
});

describe("settings", () => {
  it("lists devices and connections and confirms AnyDash data deletion", async () => {
    app = await renderApp({ path: "/settings" });
    await app.see("studio-mac");
    await app.see("Grok · Grok");
    await app.tap(Copy.deleteData);
    await app.see(Copy.deleteDataTitle);
    expect(app.text()).toContain(Copy.deleteDataLink);
    await app.tap(Copy.deleteDataLink);
    expect(app.host.opened.at(-1)).toBe("https://delino.io/account");
    await app.key(app.find(".lui-dialog-backdrop")[0], "Escape");
    expect(app.text()).not.toContain(Copy.deleteDataTitle);
  });

  it("clears the collector's native widget snapshots on desktop sign-out", async () => {
    app = await renderApp({
      path: "/settings",
      platform: HostPlatform.Desktop,
    });
    await app.see(ExtraCopy.signOut);
    const widgetId = fixtures.ids.widget(1);
    app.host.api.snapshots.set(
      widgetId,
      create(NativeWidgetSnapshotSchema, { widgetId }),
    );
    await app.tap(ExtraCopy.signOut);
    await waitFor(() => expect(app!.host.api.snapshots.size).toBe(0));
    const methods = app.host.api.calls.map((call) => call.method);
    expect(methods.lastIndexOf("PutNativeWidgetSnapshots")).toBeGreaterThan(
      methods.indexOf("Logout"),
    );
    expect(methods.indexOf("Logout")).toBeGreaterThanOrEqual(0);
  });

  it("shows the Vercel write warning before connecting on desktop", async () => {
    app = await renderApp({
      path: "/settings",
      platform: HostPlatform.Desktop,
    });
    await app.see(ExtraCopy.thisComputer);
    await app.tap(ExtraCopy.addConnection);
    await app.tap("Vercel");
    await app.see(Copy.vercelWarning);
    expect(app.text()).toContain(Copy.teamScopedToken);
  });
});
