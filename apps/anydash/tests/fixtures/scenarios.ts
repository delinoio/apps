// SPDX-License-Identifier: Apache-2.0
// Protocol fixtures for client tests and the browser preview. They mirror the
// design record's preview (section A) with every metric state.
import { create, type MessageInitShape } from "@bufbuild/protobuf";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import {
  AccountSchema,
  CollectorKind,
  ConnectionSchema,
  ConnectionState,
  CpuArchitecture,
  DashboardSchema,
  DataNotice,
  DevicePlatform,
  DeviceSchema,
  EnrollmentState,
  EnrollmentSummarySchema,
  FailureClass,
  MetricKind,
  MetricState,
  OnboardingState,
  ParserState,
  Provider,
  QueryPeriod,
  ResourceHealth,
  ResourceKind,
  SharingState,
  SpaceKind,
  SpacePermission,
  SpaceSchema,
  UsageBreakdownDimension,
  WidgetDataSchema,
  WidgetPresentation,
  WidgetSchema,
  WidgetType,
  type Connection,
  type Dashboard,
  type Device,
  MetricReadingSchema,
  type Widget,
  type WidgetData,
} from "@repo/protos/anydash/v1/anydash_pb";

export const NOW = new Date("2026-11-01T03:41:00Z"); // 12:41 in Asia/Seoul
const at = (iso: string) => timestampFromDate(new Date(iso));

export const ids = {
  user: "0192f000-0000-7000-8000-000000000001",
  personal: "0192f000-0000-7000-8000-0000000000a1",
  team: "0192f000-0000-7000-8000-0000000000b1",
  overview: "0192f000-0000-7000-8000-0000000000c1",
  teamBoard: "0192f000-0000-7000-8000-0000000000c2",
  studio: "0192f1aa-0000-7000-8000-00000000a4c1",
  buildBox: "0192f1aa-0000-7000-8000-00000000b0b0",
  laptop: "0192f1aa-0000-7000-8000-00000000c0c0",
  codex: "0192f2aa-0000-7000-8000-0000000077d0",
  claude: "0192f2aa-0000-7000-8000-0000000077d1",
  grok: "0192f2aa-0000-7000-8000-0000000077d2",
  vercel: "0192f2aa-0000-7000-8000-0000000077d3",
  supabase: "0192f2aa-0000-7000-8000-0000000077d4",
  cloudflare: "0192f2aa-0000-7000-8000-0000000077d5",
  enrollment: "0192f3aa-0000-7000-8000-000000000e01",
  widget: (index: number) =>
    `0192f4aa-0000-7000-8000-${String(index).padStart(12, "0")}`,
};

export const personalSpace = (onboarding = OnboardingState.READY) =>
  create(SpaceSchema, {
    id: ids.personal,
    kind: SpaceKind.PERSONAL,
    name: "Personal",
    onboardingState: onboarding,
    permissions: [
      SpacePermission.VIEW_DASHBOARDS,
      SpacePermission.EDIT_DASHBOARDS,
      SpacePermission.CREATE_DASHBOARDS,
      SpacePermission.DELETE_DASHBOARDS,
      SpacePermission.VIEW_SPACE_ACTIVITY,
    ],
  });
export const teamSpace = (
  permissions = [
    SpacePermission.VIEW_DASHBOARDS,
    SpacePermission.EDIT_DASHBOARDS,
  ],
) =>
  create(SpaceSchema, {
    id: ids.team,
    kind: SpaceKind.TEAM,
    name: "Team Acme",
    onboardingState: OnboardingState.READY,
    permissions,
  });

export const account = (onboarding = OnboardingState.READY) =>
  create(AccountSchema, {
    userId: ids.user,
    personalSpaceId: ids.personal,
    timeZone: "Asia/Seoul",
    onboardingState: onboarding,
  });

export const devices = (): Device[] => [
  create(DeviceSchema, {
    id: ids.studio,
    name: "studio-mac",
    platform: DevicePlatform.MACOS,
    architecture: CpuArchitecture.ARM64,
    collectorKind: CollectorKind.DESKTOP_APP,
    collectorVersion: "1.0.3",
    online: true,
    preferred: true,
    lastHeartbeatAt: at("2026-11-01T03:40:00Z"),
  }),
  create(DeviceSchema, {
    id: ids.buildBox,
    name: "build-box",
    platform: DevicePlatform.LINUX,
    architecture: CpuArchitecture.X64,
    collectorKind: CollectorKind.CLI,
    collectorVersion: "1.0.1",
    online: true,
    parserStatuses: [
      { state: ParserState.PARTIAL, skippedRecordCount: 12n },
    ],
  }),
  create(DeviceSchema, {
    id: ids.laptop,
    name: "travel-laptop",
    platform: DevicePlatform.WINDOWS,
    architecture: CpuArchitecture.X64,
    collectorKind: CollectorKind.DESKTOP_APP,
    collectorVersion: "1.0.3",
    online: false,
  }),
];

const connection = (
  id: string,
  provider: Provider,
  displayName: string,
  state = ConnectionState.OK,
  deviceName = "studio-mac",
): Connection =>
  create(ConnectionSchema, {
    id,
    provider,
    displayName,
    state,
    devices: [
      { deviceId: ids.studio, deviceName, ready: true, online: true, collecting: true },
    ],
  });
export const connections = (): Connection[] => [
  connection(ids.codex, Provider.CODEX, "Codex"),
  connection(ids.claude, Provider.CLAUDE_CODE, "Claude Code", ConnectionState.ERROR),
  connection(ids.grok, Provider.GROK, "Grok", ConnectionState.AUTH_EXPIRED),
  connection(ids.vercel, Provider.VERCEL, "Vercel"),
  connection(ids.supabase, Provider.SUPABASE, "Supabase"),
  connection(ids.cloudflare, Provider.CLOUDFLARE, "Cloudflare", ConnectionState.WAITING_FOR_FIRST_RESULT),
];

interface WidgetSpec {
  type: WidgetType;
  title: string;
  presentation?: WidgetPresentation;
  sharing?: SharingState;
  owner?: boolean;
  source?: { connectionIds?: string[]; allDevices?: boolean };
}
const widgetSpecs: WidgetSpec[] = [
  { type: WidgetType.SUBSCRIPTION_LIMITS, title: "Codex · 5-hour limit", presentation: WidgetPresentation.GAUGE, source: { connectionIds: [ids.codex] } },
  { type: WidgetType.SUBSCRIPTION_LIMITS, title: "Codex · Absolute remaining", presentation: WidgetPresentation.NUMBER, source: { connectionIds: [ids.codex] } },
  { type: WidgetType.SUBSCRIPTION_LIMITS, title: "Claude Code · Weekly limit", presentation: WidgetPresentation.GAUGE, source: { connectionIds: [ids.claude] } },
  { type: WidgetType.SUBSCRIPTION_LIMITS, title: "Grok · Credits", presentation: WidgetPresentation.GAUGE, source: { connectionIds: [ids.grok] } },
  { type: WidgetType.LOCAL_USAGE_SUMMARY, title: "Local usage · Last 7 days", presentation: WidgetPresentation.NUMBER, source: { allDevices: true } },
  { type: WidgetType.LOCAL_USAGE_BREAKDOWN, title: "Local usage · Estimated cost by model", presentation: WidgetPresentation.TABLE, source: { allDevices: true } },
  { type: WidgetType.DEPLOYMENT_COST, title: "Vercel · Month to date", presentation: WidgetPresentation.NUMBER, source: { connectionIds: [ids.vercel] } },
  { type: WidgetType.DEPLOYMENT_USAGE, title: "Supabase API requests", presentation: WidgetPresentation.NUMBER, sharing: SharingState.ENDED, owner: false },
  { type: WidgetType.DEPLOYMENT_USAGE, title: "Cloudflare · Workers invocations", presentation: WidgetPresentation.NUMBER, source: { connectionIds: [ids.cloudflare] } },
];

export function makeWidgets(specs = widgetSpecs): Widget[] {
  return specs.map((spec, index) =>
    create(WidgetSchema, {
      id: ids.widget(index + 1),
      type: spec.type,
      layout: {
        x: (index % 3) * 4,
        y: Math.floor(index / 3) * 2,
        width: 4,
        height: 2,
        title: spec.title,
        presentation: spec.presentation ?? WidgetPresentation.NUMBER,
      },
      query:
        spec.type === WidgetType.LOCAL_USAGE_SUMMARY || spec.type === WidgetType.LOCAL_USAGE_BREAKDOWN
          ? {
              period: QueryPeriod.LAST_7_DAYS,
              breakdown:
                spec.type === WidgetType.LOCAL_USAGE_BREAKDOWN
                  ? UsageBreakdownDimension.MODEL
                  : undefined,
            }
          : {},
      source: spec.owner === false ? undefined : spec.source,
      callerIsDataOwner: spec.owner !== false,
      dataOwnerUserId: spec.owner === false ? "0192f000-0000-7000-8000-0000000000ff" : ids.user,
      sharingState: spec.sharing ?? SharingState.PRIVATE,
    }),
  );
}

export function makeDashboard(
  spaceId = ids.personal,
  version = 7n,
  widgets = makeWidgets(),
  id = ids.overview,
): Dashboard {
  return create(DashboardSchema, {
    id,
    spaceId,
    name: "Overview",
    version,
    widgets,
    createdByUserId: ids.user,
  });
}

const reading = (init: MessageInitShape<typeof MetricReadingSchema>) => init;

export function makeWidgetData(widgets: Widget[] = makeWidgets()): WidgetData[] {
  const observed = at("2026-11-01T03:41:00Z");
  const content: Record<number, WidgetData["content"]> = {
    0: {
      case: "metricPanel",
      value: {
        readings: [
          reading({ connectionId: ids.codex, provider: Provider.CODEX, kind: MetricKind.SUBSCRIPTION_REMAINING_PERCENT, dimension: "five_hour", current: { state: MetricState.VALUE, value: { case: "percent", value: 62 } }, observedAt: observed }),
          reading({ connectionId: ids.codex, provider: Provider.CODEX, kind: MetricKind.SUBSCRIPTION_RESET_AT, dimension: "five_hour", current: { state: MetricState.VALUE, value: { case: "time", value: at("2026-11-01T05:20:00Z") } }, observedAt: observed }),
        ],
      } as never,
    },
    1: {
      case: "metricPanel",
      value: {
        readings: [
          reading({ connectionId: ids.codex, provider: Provider.CODEX, kind: MetricKind.SUBSCRIPTION_REMAINING_AMOUNT, dimension: "five_hour", current: { state: MetricState.NOT_PROVIDED }, observedAt: observed }),
        ],
      } as never,
    },
    2: {
      case: "metricPanel",
      value: {
        readings: [
          reading({ connectionId: ids.claude, provider: Provider.CLAUDE_CODE, kind: MetricKind.SUBSCRIPTION_REMAINING_PERCENT, dimension: "seven_day", current: { state: MetricState.FAILED }, lastSuccess: { state: MetricState.VALUE, value: { case: "percent", value: 71 } }, lastSuccessAt: at("2026-11-01T02:52:00Z"), failureClass: FailureClass.NETWORK, observedAt: observed }),
        ],
      } as never,
    },
    3: {
      case: "metricPanel",
      value: {
        readings: [
          reading({ connectionId: ids.grok, provider: Provider.GROK, kind: MetricKind.SUBSCRIPTION_REMAINING_PERCENT, dimension: "credits", current: { state: MetricState.AUTH_EXPIRED }, lastSuccess: { state: MetricState.VALUE, value: { case: "percent", value: 54 } }, lastSuccessAt: at("2026-10-31T09:05:00Z"), failureClass: FailureClass.AUTH_EXPIRED, observedAt: observed }),
        ],
      } as never,
    },
    4: {
      case: "usageSummary",
      value: {
        totals: {
          sessionCount: 120n,
          tokens: { inputTokens: 30_000_000n, outputTokens: 12_000_000n, cacheReadTokens: 6_000_000n, cacheWriteTokens: 200_000n },
          estimatedCost: { state: MetricState.VALUE, amountUsd: "214.300000" },
        },
      } as never,
    },
    5: {
      case: "usageBreakdown",
      value: {
        dimension: UsageBreakdownDimension.MODEL,
        rows: [
          { model: "gpt-6-luna", totals: { tokens: { inputTokens: 1_000_000n }, estimatedCost: { state: MetricState.CANNOT_BE_CALCULATED } } },
        ],
        total: { tokens: { inputTokens: 1_000_000n }, estimatedCost: { state: MetricState.CANNOT_BE_CALCULATED } },
      } as never,
    },
    6: {
      case: "metricPanel",
      value: {
        readings: [
          reading({ connectionId: ids.vercel, provider: Provider.VERCEL, kind: MetricKind.DEPLOYMENT_BILLED_COST, dimension: "total", current: { state: MetricState.VALUE, value: { case: "money", value: { currencyCode: "USD", amount: "41.80" } } }, observedAt: observed }),
          reading({ connectionId: ids.vercel, provider: Provider.VERCEL, kind: MetricKind.DEPLOYMENT_PLAN, dimension: "plan", current: { state: MetricState.VALUE, value: { case: "text", value: "Hobby" } }, observedAt: observed }),
        ],
      } as never,
    },
  };
  const notices: Record<number, DataNotice[]> = {
    4: [DataNotice.POSSIBLE_CROSS_DEVICE_DUPLICATION, DataNotice.PARTIAL_AGGREGATION, DataNotice.ESTIMATED_COST],
  };
  return widgets
    .map((widget, index) => {
      // The Cloudflare widget has no data yet and renders the loading state.
      if (index === 8) return undefined;
      return create(WidgetDataSchema, {
        widgetId: widget.id,
        type: widget.type,
        sharingState: widget.sharingState,
        generatedAt: observed,
        timeZone: "Asia/Seoul",
        notices: widget.sharingState === SharingState.ENDED ? [] : (notices[index] ?? []),
        summary:
          widget.sharingState === SharingState.ENDED
            ? undefined
            : { title: widget.layout?.title ?? "", dataUpdatedAt: observed },
        content: widget.sharingState === SharingState.ENDED ? { case: undefined } : (content[index] ?? { case: undefined }),
      });
    })
    .filter((entry): entry is WidgetData => !!entry);
}

export const resourceData = () =>
  create(WidgetDataSchema, {
    widgetId: ids.widget(20),
    type: WidgetType.DEPLOYMENT_RESOURCES,
    sharingState: SharingState.PRIVATE,
    content: {
      case: "resourceList",
      value: {
        resources: [
          { connectionId: ids.vercel, provider: Provider.VERCEL, resourceId: "prj_1", kind: ResourceKind.VERCEL_PROJECT, displayName: "site", health: ResourceHealth.HEALTHY, providerStatus: "READY", statusState: MetricState.VALUE },
          { connectionId: ids.vercel, provider: Provider.VERCEL, resourceId: "prj_2", kind: ResourceKind.VERCEL_PROJECT, displayName: "docs", health: ResourceHealth.HEALTHY, providerStatus: "READY", statusState: MetricState.VALUE },
        ],
      },
    },
  });

export const enrollment = (overrides: Partial<{ state: EnrollmentState; remainingAttempts: number; minutes: number }> = {}) =>
  create(EnrollmentSummarySchema, {
    enrollmentId: ids.enrollment,
    state: overrides.state ?? EnrollmentState.PENDING,
    deviceName: "studio-mac",
    platform: DevicePlatform.MACOS,
    architecture: CpuArchitecture.ARM64,
    collectorKind: CollectorKind.DESKTOP_APP,
    collectorVersion: "1.0.3",
    // Relative to the real clock so the approval countdown is live in tests.
    expiresAt: timestampFromDate(new Date(Date.now() + (overrides.minutes ?? 8.7) * 60_000)),
    remainingAttempts: overrides.remainingAttempts ?? 4,
  });
