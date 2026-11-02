// SPDX-License-Identifier: Apache-2.0
// In-memory AnyDash API and collector used by component tests and the browser
// preview. It speaks the same binary Protobuf and Connect error format that the
// hosts forward, so the real HostTransport and connect-query run unchanged.
import {
  create,
  fromBinary,
  type MessageInitShape,
  toBinary,
  type DescMessage,
  type DescService,
} from "@bufbuild/protobuf";
import { base64Encode } from "@bufbuild/protobuf/wire";
import { durationFromMs, timestampFromDate } from "@bufbuild/protobuf/wkt";
import {
  AccountService,
  CollectorRunState,
  ConnectionService,
  DashboardService,
  DeviceService,
  EnrollmentState,
  ErrorDetailSchema,
  ErrorReason,
  KeySource,
  LocalCollectorService,
  OnboardingState,
  QuotaKind,
  RateLimitScope,
  SharingState,
  SpacePermission,
  WidgetDataService,
  type Dashboard,
  type NativeWidgetSnapshot,
  type Space,
  type WidgetData,
} from "@repo/protos/anydash/v1/anydash_pb";
import * as fixtures from "./scenarios";

export interface MockOptions {
  onboarding?: OnboardingState;
  team?: boolean;
  teamPermissions?: SpacePermission[];
  /** Personal dashboards to start with; the default has one with every state. */
  personalDashboards?: boolean;
  /** UpdateWidgetLayout rejects once with VERSION_CONFLICT after another save. */
  conflictOnLayout?: boolean;
  /** Widgets whose sharing the owner ended; returned as ENDED. */
  endedWidgetIds?: string[];
  updateRequired?: boolean;
  quotaOnCreate?: boolean;
  dashboardError?: ErrorReason;
  /** Dashboard data never resolves, to show the loading state. */
  pendingData?: boolean;
  /** Rejects the first ApproveEnrollment code. */
  codeMismatch?: boolean;
  enrollmentState?: EnrollmentState;
}

export class MockError extends Error {
  constructor(
    readonly code: string,
    readonly status: number,
    readonly reason: ErrorReason,
    readonly detail: MessageInitShape<typeof ErrorDetailSchema> = {},
  ) {
    super(ErrorReason[reason].toLowerCase().replace(/_/g, " "));
  }
}
const conflict = (current: bigint) =>
  new MockError("aborted", 409, ErrorReason.VERSION_CONFLICT, { currentVersion: current });

export interface RecordedCall {
  service: string;
  method: string;
  request: unknown;
}

let correlation = 0;
export class MockApi {
  calls: RecordedCall[] = [];
  snapshots = new Map<string, NativeWidgetSnapshot>();
  /** Connections with a credential stored in the local collector. */
  localCredentials = new Set<string>([fixtures.ids.claude]);
  personal: Space;
  team?: Space;
  dashboards = new Map<string, Dashboard>();
  private conflictPending: boolean;
  private mismatchPending: boolean;
  private enrollmentAttempts = 4;
  private enrollmentState: EnrollmentState;

  constructor(readonly options: MockOptions = {}) {
    this.personal = fixtures.personalSpace(options.onboarding);
    if (options.team) this.team = fixtures.teamSpace(options.teamPermissions);
    if (options.personalDashboards !== false && options.onboarding !== OnboardingState.DEVICE_REQUIRED)
      this.dashboards.set(fixtures.ids.overview, fixtures.makeDashboard());
    if (options.team)
      this.dashboards.set(
        fixtures.ids.teamBoard,
        fixtures.makeDashboard(fixtures.ids.team, 3n, fixtures.makeWidgets().slice(0, 3), fixtures.ids.teamBoard),
      );
    this.conflictPending = options.conflictOnLayout === true;
    this.mismatchPending = options.codeMismatch === true;
    this.enrollmentState = options.enrollmentState ?? EnrollmentState.PENDING;
  }

  private data(dashboard: Dashboard): WidgetData[] {
    const ended = new Set(this.options.endedWidgetIds ?? []);
    return fixtures.makeWidgetData(dashboard.widgets).map((entry) =>
      ended.has(entry.widgetId)
        ? { ...entry, sharingState: SharingState.ENDED, summary: undefined, notices: [], content: { case: undefined } } as WidgetData
        : entry,
    );
  }

  private dashboard(id: string) {
    const found = this.dashboards.get(id);
    if (!found) throw new MockError("not_found", 404, ErrorReason.NOT_FOUND, { field: "dashboard_id" });
    return found;
  }

  private bump(dashboard: Dashboard, expected: bigint | undefined) {
    if (expected !== undefined && expected !== dashboard.version) throw conflict(dashboard.version);
    dashboard.version += 1n;
    return dashboard;
  }

  private handlers(): Record<string, (request: never) => unknown> {
    const o = this.options;
    const spaces = () => [this.personal, ...(this.team ? [this.team] : [])];
    return {
      "AccountService/ConnectAccount": () => ({ account: fixtures.account(this.personal.onboardingState) }),
      "AccountService/GetAccount": () => ({ account: fixtures.account(this.personal.onboardingState) }),
      "AccountService/UpdateAccountSettings": (request: { timeZone: string }) => ({
        account: { ...fixtures.account(), timeZone: request.timeZone },
      }),
      "AccountService/ListActivity": () => ({
        events: [{ id: fixtures.ids.widget(90), kind: 1, occurredAt: timestampFromDate(fixtures.NOW), clientKind: 1 }],
      }),
      "AccountService/DeleteAccountData": () => ({ purgeDeadline: timestampFromDate(fixtures.NOW) }),
      "DashboardService/ListSpaces": () => ({ spaces: spaces() }),
      "DashboardService/ListDashboards": (request: { spaceId: string }) => {
        if (request.spaceId === this.personal.id && this.personal.onboardingState === OnboardingState.DEVICE_REQUIRED)
          throw new MockError("failed_precondition", 400, ErrorReason.ONBOARDING_REQUIRED);
        return {
          dashboards: [...this.dashboards.values()]
            .filter((entry) => entry.spaceId === request.spaceId)
            .map((entry) => ({ id: entry.id, spaceId: entry.spaceId, name: entry.name, version: entry.version, widgetCount: entry.widgets.length })),
        };
      },
      "DashboardService/GetDashboard": (request: { dashboardId: string }) => {
        if (o.dashboardError === ErrorReason.PERMISSION_DENIED)
          throw new MockError("permission_denied", 403, ErrorReason.PERMISSION_DENIED);
        if (o.dashboardError === ErrorReason.SPACE_INACTIVE)
          throw new MockError("permission_denied", 403, ErrorReason.SPACE_INACTIVE);
        if (o.dashboardError === ErrorReason.INTERNAL)
          throw new MockError("internal", 500, ErrorReason.INTERNAL);
        const dashboard = this.dashboard(request.dashboardId);
        const ended = new Set(o.endedWidgetIds ?? []);
        return {
          dashboard: {
            ...dashboard,
            widgets: dashboard.widgets.map((widget) =>
              ended.has(widget.id) ? { ...widget, sharingState: SharingState.ENDED } : widget,
            ),
          },
        };
      },
      "DashboardService/CreateDashboard": (request: { spaceId: string; name: string }) => {
        if (o.quotaOnCreate)
          throw new MockError("resource_exhausted", 429, ErrorReason.QUOTA_EXCEEDED, {
            quota: { kind: QuotaKind.DASHBOARDS_PER_SPACE, limit: 50, used: 50 },
          });
        const dashboard = fixtures.makeDashboard(request.spaceId, 1n, [], fixtures.ids.widget(70 + this.dashboards.size));
        dashboard.name = request.name;
        this.dashboards.set(dashboard.id, dashboard);
        return { dashboard };
      },
      "DashboardService/RenameDashboard": (request: { dashboardId: string; expectedVersion: bigint; name: string }) => {
        const dashboard = this.bump(this.dashboard(request.dashboardId), request.expectedVersion);
        dashboard.name = request.name;
        return { dashboard };
      },
      "DashboardService/DeleteDashboard": (request: { dashboardId: string }) => {
        this.dashboards.delete(request.dashboardId);
        return { purgeDeadline: timestampFromDate(fixtures.NOW) };
      },
      "DashboardService/UpdateWidgetLayout": (request: {
        dashboardId: string;
        expectedVersion: bigint;
        updates: { widgetId: string; layout: { x: number; y: number; width: number; height: number } }[];
      }) => {
        const dashboard = this.dashboard(request.dashboardId);
        if (this.conflictPending) {
          // Another member saved first.
          this.conflictPending = false;
          dashboard.version += 1n;
          dashboard.name = "Overview";
          throw conflict(dashboard.version);
        }
        this.bump(dashboard, request.expectedVersion);
        for (const update of request.updates) {
          const widget = dashboard.widgets.find((entry) => entry.id === update.widgetId);
          if (widget?.layout) Object.assign(widget.layout, update.layout);
        }
        return { dashboard };
      },
      "DashboardService/AddWidget": (request: { dashboardId: string; expectedVersion: bigint; type: number; layout: never }) => {
        const dashboard = this.bump(this.dashboard(request.dashboardId), request.expectedVersion);
        const widget = fixtures.makeWidgets([{ type: request.type, title: "" }])[0];
        widget.id = fixtures.ids.widget(50 + dashboard.widgets.length);
        widget.layout = request.layout;
        widget.sharingState = dashboard.spaceId === this.personal.id ? SharingState.PRIVATE : SharingState.SHARED;
        dashboard.widgets.push(widget);
        return { dashboard, widgetId: widget.id };
      },
      "DashboardService/RemoveWidget": (request: { dashboardId: string; expectedVersion: bigint; widgetId: string }) => {
        const dashboard = this.bump(this.dashboard(request.dashboardId), request.expectedVersion);
        dashboard.widgets = dashboard.widgets.filter((widget) => widget.id !== request.widgetId);
        return { dashboard };
      },
      "DashboardService/RevokeWidgetSharing": (request: { dashboardId: string; widgetId: string }) => {
        const dashboard = this.bump(this.dashboard(request.dashboardId), undefined);
        const widget = dashboard.widgets.find((entry) => entry.id === request.widgetId);
        if (widget) widget.sharingState = SharingState.ENDED;
        return { dashboard };
      },
      "WidgetDataService/QueryDashboardData": (request: { dashboardId: string }) => {
        if (o.pendingData) return new Promise(() => {});
        const dashboard = this.dashboard(request.dashboardId);
        return { dashboardVersion: dashboard.version, widgets: this.data(dashboard) };
      },
      "DeviceService/ListDevices": () => ({ devices: fixtures.devices() }),
      "DeviceService/RequestDeviceSync": () => ({ nextAllowedAt: timestampFromDate(new Date(Date.now() + 60_000)) }),
      "DeviceService/SetPreferredDevice": (request: { deviceId: string }) => ({ preferredDeviceId: request.deviceId }),
      "DeviceService/DeleteDevice": () => ({ purgeDeadline: timestampFromDate(fixtures.NOW) }),
      "DeviceService/GetEnrollment": () => ({
        enrollment: { ...fixtures.enrollment({ state: this.enrollmentState }), remainingAttempts: this.enrollmentAttempts },
      }),
      "DeviceService/ApproveEnrollment": () => {
        if (this.mismatchPending) {
          this.mismatchPending = false;
          this.enrollmentAttempts -= 1;
          throw new MockError("invalid_argument", 400, ErrorReason.ENROLLMENT_CODE_MISMATCH, { field: "user_code" });
        }
        this.enrollmentState = EnrollmentState.APPROVED;
        // The first approved device completes personal onboarding.
        this.personal = { ...this.personal, onboardingState: OnboardingState.READY };
        return { device: fixtures.devices()[0] };
      },
      "DeviceService/RejectEnrollment": () => {
        this.enrollmentState = EnrollmentState.REJECTED;
        return {};
      },
      "ConnectionService/ListConnections": () => ({ connections: fixtures.connections() }),
      "ConnectionService/RequestConnectionRefresh": () => ({
        nextAllowedAt: timestampFromDate(new Date(Date.now() + 60_000)),
      }),
      "ConnectionService/DeleteConnection": () => ({ purgeDeadline: timestampFromDate(fixtures.NOW) }),
      "LocalCollectorService/GetCollectorStatus": () => ({
        status: {
          runState: CollectorRunState.RUNNING,
          deviceId: fixtures.ids.studio,
          collectorVersion: "1.0.3",
          keySource: KeySource.OS_SECURE_STORE,
          connections: [fixtures.ids.claude, ...this.localCredentials]
            .filter((id, index, all) => all.indexOf(id) === index)
            .map((connectionId) => ({
              connectionId,
              nextAttemptAt: timestampFromDate(new Date(fixtures.NOW.getTime() + 4 * 60_000)),
              credentialPresent: this.localCredentials.has(connectionId),
            })),
        },
      }),
      "LocalCollectorService/PutNativeWidgetSnapshots": (request: { snapshots: NativeWidgetSnapshot[]; replaceAll: boolean }) => {
        if (request.replaceAll) this.snapshots.clear();
        for (const snapshot of request.snapshots) this.snapshots.set(snapshot.widgetId, snapshot);
        return {};
      },
      "LocalCollectorService/ListNativeWidgetSnapshots": () => ({ snapshots: [...this.snapshots.values()] }),
      "LocalCollectorService/ListSourcePaths": () => ({
        paths: [{ tool: 1, path: "~/.codex/sessions", origin: 1, enabled: true, exists: true }],
      }),
      "LocalCollectorService/RequestLocalSync": () => {
        throw new MockError("resource_exhausted", 429, ErrorReason.RATE_LIMITED, {
          retryAfter: durationFromMs(42_000),
          rateLimitScope: RateLimitScope.MANUAL_REFRESH,
        });
      },
      "LocalCollectorService/Logout": () => ({}),
      "LocalCollectorService/AddConnectionCredential": (request: { connectionId: string }) => {
        this.localCredentials.add(request.connectionId);
        return { connectionId: request.connectionId };
      },
      "LocalCollectorService/RemoveConnectionCredential": (request: { connectionId: string }) => {
        this.localCredentials.delete(request.connectionId);
        return {};
      },
    };
  }

  /** Answers one forwarded request with an HTTP status and body bytes. */
  async handle(service: string, method: string, body: Uint8Array) {
    const name = service.replace(/^anydash\.v1\./, "");
    const descriptor = (
      {
        AccountService,
        DashboardService,
        WidgetDataService,
        DeviceService,
        ConnectionService,
        LocalCollectorService,
      } as Record<string, DescService>
    )[name];
    const schema = descriptor?.methods.find((entry) => entry.name === method);
    const id = `0192f9aa-0000-7000-8000-${String(++correlation).padStart(12, "0")}`;
    try {
      if (!descriptor || !schema) throw new MockError("unimplemented", 404, ErrorReason.UNSPECIFIED);
      if (this.options.updateRequired && name !== "LocalCollectorService")
        throw new MockError("failed_precondition", 400, ErrorReason.UPDATE_REQUIRED, {
          minimumClientVersion: "9.0.0",
        });
      const request = fromBinary(schema.input as DescMessage, body);
      this.calls.push({ service: name, method, request });
      const handler = this.handlers()[`${name}/${method}`];
      if (!handler) throw new MockError("unimplemented", 404, ErrorReason.UNSPECIFIED);
      const result = await handler(request as never);
      return {
        status: 200,
        body: toBinary(schema.output as DescMessage, create(schema.output as DescMessage, result as never)),
        correlationId: id,
      };
    } catch (error) {
      const failure = error instanceof MockError ? error : new MockError("internal", 500, ErrorReason.INTERNAL);
      const detail = create(ErrorDetailSchema, failure.detail);
      detail.reason = failure.reason;
      detail.correlationId = id;
      const json = {
        code: failure.code,
        message: failure.message,
        details: [{ type: ErrorDetailSchema.typeName, value: base64Encode(toBinary(ErrorDetailSchema, detail)).replace(/=+$/, "") }],
      };
      return { status: failure.status, body: new TextEncoder().encode(JSON.stringify(json)), correlationId: id };
    }
  }

  ended(id: string) {
    this.options.endedWidgetIds = [...(this.options.endedWidgetIds ?? []), id];
  }
}
