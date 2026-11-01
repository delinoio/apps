// SPDX-License-Identifier: Apache-2.0
import { pick } from "../format";
import { create } from "@bufbuild/protobuf";
import { timestampDate, type Timestamp } from "@bufbuild/protobuf/wkt";
import {
  DataNotice,
  FailureClass,
  MetricKind,
  MetricState,
  ParserState,
  ResourceHealth,
  SharingState,
  UsageTotalsSchema,
  WidgetPresentation,
  WidgetType,
  type Connection,
  type Device,
  type MetricReading,
  type MetricValue,
  type TokenCounts,
  type UsageTotals,
  type Widget,
  type WidgetData,
} from "@repo/protos/anydash/v1/anydash_pb";
import { Copy } from "../copy";
import {
  dimensionLabel,
  formatCompact,
  formatCount,
  formatDecimal,
  formatMoney,
  formatPercent,
  formatTime,
  providerCli,
  providerName,
  toolName,
  usesLocalLogin,
} from "../format";

export enum WidgetState {
  Loading = "loading",
  Value = "value",
  NotProvided = "not-provided",
  Failed = "failed",
  AuthExpired = "auth-expired",
  Partial = "partial",
  CannotBeCalculated = "cannot-be-calculated",
  SharingEnded = "sharing-ended",
}

export enum BadgeTone {
  Danger = "danger",
  Warning = "warning",
  Info = "info",
  Success = "success",
  Neutral = "neutral",
}

export enum LineStyle {
  Muted = "muted",
  Stale = "stale",
}

export interface ViewLine {
  text: string;
  style: LineStyle;
  /** Inline code fragment, for example a model identifier. */
  code?: string;
  /** Emphasized fragment, for example a device name. */
  strong?: string;
  /** Render after the state badges even when the value leads. */
  afterBadges?: boolean;
}
export interface ViewBadge {
  tone: BadgeTone;
  label: string;
}
export interface ViewRow {
  label: string;
  value: string;
  muted?: boolean;
}
export interface WidgetView {
  id: string;
  title: string;
  state: WidgetState;
  /** Primary value; `muted` renders the grey Not provided treatment. */
  primary?: { text: string; muted?: boolean; stale?: boolean };
  /** Used percentage for the subscription progress bar. */
  usedPercent?: number;
  badges: ViewBadge[];
  lines: ViewLine[];
  rows: ViewRow[];
  /** Bar heights from 0 to 1 for chart presentations. */
  bars: number[];
  updatedAt?: Date;
  /** Screen readers read the state label, then the value, then the time. */
  accessibilityLabel: string;
}

export interface ViewContext {
  timeZone: string;
  now: Date;
  devices: Device[];
  connections: Connection[];
  /** Next collector attempt per connection, known on desktop only. */
  nextAttempt?: Map<string, Timestamp | undefined>;
}

export const defaultTitle = (type: WidgetType) =>
  pick(
    {
      [WidgetType.SUBSCRIPTION_LIMITS]: "Subscription limits",
      [WidgetType.SUBSCRIPTION_HISTORY]: "Subscription history",
      [WidgetType.LOCAL_USAGE_SUMMARY]: "Local usage",
      [WidgetType.LOCAL_USAGE_SERIES]: "Local usage over time",
      [WidgetType.LOCAL_USAGE_BREAKDOWN]: "Local usage breakdown",
      [WidgetType.DEPLOYMENT_RESOURCES]: "Deployments",
      [WidgetType.DEPLOYMENT_USAGE]: "Deployment usage",
      [WidgetType.DEPLOYMENT_COST]: "Deployment cost",
      [WidgetType.METRIC_HISTORY]: "Metric history",
    },
    type,
    "Widget",
  );

/** Display title, including the shared-by prefix for other owners' widgets. */
export function widgetTitle(widget: Widget, data?: WidgetData) {
  const base =
    widget.layout?.title ||
    (data?.sharingState !== SharingState.ENDED ? data?.summary?.title : "") ||
    defaultTitle(widget.type);
  // The protocol exposes only the owner's user ID, not a display name, so the
  // record's "Shared by {name}" uses a neutral owner reference.
  return !widget.callerIsDataOwner &&
    widget.sharingState !== SharingState.PRIVATE &&
    widget.sharingState !== SharingState.UNSPECIFIED
    ? `Shared by a team member · ${base}`
    : base;
}

const tokenTotal = (tokens: TokenCounts | undefined) =>
  (tokens?.inputTokens ?? 0n) +
  (tokens?.outputTokens ?? 0n) +
  (tokens?.cacheReadTokens ?? 0n) +
  (tokens?.cacheWriteTokens ?? 0n);

export const formatTokens = (tokens: TokenCounts | undefined) =>
  `${formatCompact(tokenTotal(tokens))} tokens`;

export function formatMetricValue(
  value: MetricValue | undefined,
  kind: MetricKind,
  unit: string,
  timeZone: string,
  now: Date,
) {
  switch (value?.value.case) {
    case "percent":
      return kind === MetricKind.SUBSCRIPTION_REMAINING_PERCENT
        ? Copy.percentLeft(formatPercent(value.value.value))
        : `${formatPercent(value.value.value)}%`;
    case "money":
      return formatMoney(value.value.value);
    case "decimal":
      return `${formatDecimal(value.value.value)}${unit ? ` ${unit}` : ""}`;
    case "integer":
      return `${formatCount(value.value.value)}${unit ? ` ${unit}` : ""}`;
    case "time":
      return formatTime(value.value.value, timeZone, now);
    case "duration": {
      const hours = Number(value.value.value.seconds) / 3600;
      return hours >= 24 ? `${Math.round(hours / 24)} days` : `${hours} hours`;
    }
    case "text":
      return value.value.value;
    case "flag":
      return value.value.value ? "On" : "Off";
    default:
      return "";
  }
}

const date = (value: Timestamp | undefined) =>
  value ? timestampDate(value) : undefined;

const connectionDevice = (connection: Connection | undefined) =>
  connection?.devices.find((device) => device.collecting) ??
  connection?.devices[0];

function labelFor(
  view: Omit<WidgetView, "accessibilityLabel">,
  ctx: ViewContext,
) {
  const parts: string[] = [];
  const stateLabel =
    view.state === WidgetState.Value || view.state === WidgetState.Loading
      ? ""
      : view.badges[0]?.label || (view.primary?.muted ? view.primary.text : "");
  if (view.state === WidgetState.Loading) parts.push(Copy.loading);
  if (stateLabel) parts.push(stateLabel);
  if (view.primary && view.primary.text !== stateLabel)
    parts.push(view.primary.text);
  for (const line of view.lines) parts.push(line.text);
  for (const row of view.rows) parts.push(`${row.label}: ${row.value}`);
  if (view.updatedAt && view.state !== WidgetState.SharingEnded)
    parts.push(
      Copy.nativeUpdated(formatTime(view.updatedAt, ctx.timeZone, ctx.now)),
    );
  return `${view.title}. ${parts.join(". ")}`.replace(/\.\./g, ".");
}

function finish(
  view: Omit<WidgetView, "accessibilityLabel">,
  ctx: ViewContext,
): WidgetView {
  return { ...view, accessibilityLabel: labelFor(view, ctx) };
}

function readingState(reading: MetricReading) {
  return reading.current?.state ?? MetricState.UNSPECIFIED;
}

const subscriptionKinds = [
  MetricKind.SUBSCRIPTION_REMAINING_PERCENT,
  MetricKind.SUBSCRIPTION_USED_PERCENT,
];

/** Picks the reading that drives the large value of a metric panel. */
function primaryReading(type: WidgetType, readings: MetricReading[]) {
  const preferred =
    type === WidgetType.SUBSCRIPTION_LIMITS
      ? subscriptionKinds
      : type === WidgetType.DEPLOYMENT_COST
        ? [
            MetricKind.DEPLOYMENT_BILLED_COST,
            MetricKind.DEPLOYMENT_EFFECTIVE_COST,
          ]
        : [
            MetricKind.DEPLOYMENT_USAGE_QUANTITY,
            MetricKind.DEPLOYMENT_DISK_UTILIZATION,
          ];
  for (const kind of preferred) {
    const match = readings.find((reading) => reading.kind === kind);
    if (match) return match;
  }
  return readings[0];
}

function subscriptionParts(
  reading: MetricReading,
  value: MetricValue | undefined,
  readings: MetricReading[],
  ctx: ViewContext,
) {
  if (value?.value.case !== "percent") return undefined;
  // Remaining is always 100 − provider-reported used percentage.
  const used =
    reading.kind === MetricKind.SUBSCRIPTION_REMAINING_PERCENT
      ? 100 - value.value.value
      : value.value.value;
  const remaining = 100 - used;
  const reset = readings.find(
    (entry) =>
      entry.kind === MetricKind.SUBSCRIPTION_RESET_AT &&
      entry.connectionId === reading.connectionId &&
      entry.dimension === reading.dimension &&
      entry.current?.value.case === "time",
  );
  const resetTime =
    reset?.current?.value.case === "time"
      ? formatTime(reset.current.value.value, ctx.timeZone, ctx.now)
      : "";
  return {
    used: Math.max(0, Math.min(100, used)),
    text: Copy.percentLeft(formatPercent(remaining)),
    detail: resetTime
      ? Copy.percentUsedResets(formatPercent(used), resetTime)
      : Copy.percentUsed(formatPercent(used)),
  };
}

function metricPanelView(
  widget: Widget,
  data: WidgetData,
  readings: MetricReading[],
  base: Omit<WidgetView, "accessibilityLabel" | "state">,
  ctx: ViewContext,
): Omit<WidgetView, "accessibilityLabel"> {
  const reading = primaryReading(widget.type, readings);
  if (!reading)
    return {
      ...base,
      state: WidgetState.NotProvided,
      primary: { text: Copy.notProvided, muted: true },
    };
  const provider = providerName(reading.provider);
  const connection = ctx.connections.find(
    (entry) => entry.id === reading.connectionId,
  );
  const state = readingState(reading);
  const describe = (value: MetricValue | undefined) =>
    widget.type === WidgetType.SUBSCRIPTION_LIMITS
      ? subscriptionParts(reading, value, readings, ctx)
      : value
        ? {
            used: undefined,
            text: formatMetricValue(
              value,
              reading.kind,
              reading.unit,
              ctx.timeZone,
              ctx.now,
            ),
            detail: undefined,
          }
        : undefined;
  const rows: ViewRow[] =
    widget.layout?.presentation === WidgetPresentation.LIST ||
    widget.layout?.presentation === WidgetPresentation.TABLE
      ? readings
          .filter(
            (entry) =>
              entry !== reading &&
              entry.kind !== MetricKind.SUBSCRIPTION_RESET_AT,
          )
          .map((entry) => ({
            label: `${providerName(entry.provider)} · ${dimensionLabel(entry.dimension)}`,
            value:
              readingState(entry) === MetricState.NOT_PROVIDED
                ? Copy.notProvided
                : describe(entry.current)?.text ||
                  formatMetricValue(
                    entry.current,
                    entry.kind,
                    entry.unit,
                    ctx.timeZone,
                    ctx.now,
                  ),
            muted: readingState(entry) !== MetricState.VALUE,
          }))
      : [];
  const updatedAt = date(reading.observedAt) ?? base.updatedAt;
  switch (state) {
    case MetricState.VALUE: {
      const parts = describe(reading.current);
      const lines: ViewLine[] = parts?.detail
        ? [{ text: parts.detail, style: LineStyle.Muted }]
        : [];
      if (
        widget.type === WidgetType.DEPLOYMENT_COST &&
        reading.current?.value.case === "money"
      ) {
        const plan = readings.find(
          (entry) =>
            entry.kind === MetricKind.DEPLOYMENT_PLAN &&
            entry.connectionId === reading.connectionId &&
            entry.current?.value.case === "text",
        );
        const currency = reading.current.value.value.currencyCode;
        lines.push({
          text: `${Copy.billedCost(currency || "Provider currency", provider)}${
            plan?.current?.value.case === "text"
              ? ` · ${plan.current.value.value}`
              : ""
          }`,
          style: LineStyle.Muted,
        });
      }
      return {
        ...base,
        state: WidgetState.Value,
        primary: { text: parts?.text ?? "" },
        usedPercent: parts?.used,
        lines,
        rows,
        updatedAt,
      };
    }
    case MetricState.NOT_PROVIDED:
      return {
        ...base,
        state: WidgetState.NotProvided,
        primary: { text: Copy.notProvided, muted: true },
        lines: [
          { text: Copy.notProvidedDetail(provider), style: LineStyle.Muted },
        ],
        rows,
        updatedAt,
      };
    case MetricState.AUTH_EXPIRED: {
      const device = connectionDevice(connection)?.deviceName;
      const cli = providerCli(reading.provider);
      const guidance =
        usesLocalLogin(reading.provider) && cli && device
          ? {
              text: Copy.authExpiredCli(cli, device),
              style: LineStyle.Muted,
              strong: device,
            }
          : { text: Copy.authExpiredToken(provider), style: LineStyle.Muted };
      const last = describe(reading.lastSuccess);
      const lines: ViewLine[] = [guidance];
      if (last?.text && reading.lastSuccessAt)
        lines.push({
          text: Copy.lastValue(
            last.text,
            formatTime(reading.lastSuccessAt, ctx.timeZone, ctx.now),
          ),
          style: LineStyle.Stale,
        });
      return {
        ...base,
        state: WidgetState.AuthExpired,
        badges: [{ tone: BadgeTone.Warning, label: Copy.authExpired }],
        lines,
        rows,
        updatedAt: date(reading.lastSuccessAt),
      };
    }
    case MetricState.CANNOT_BE_CALCULATED:
      return {
        ...base,
        state: WidgetState.CannotBeCalculated,
        primary: { text: Copy.cannotBeCalculated, muted: true },
        rows,
        updatedAt,
      };
    default: {
      // FAILED, PARTIAL and unknown states keep the last successful value.
      const last = describe(reading.lastSuccess);
      const lastAt = reading.lastSuccessAt;
      const next = ctx.nextAttempt?.get(reading.connectionId);
      const lines: ViewLine[] = [];
      if (reading.failureClass === FailureClass.RATE_LIMITED && next)
        lines.push({
          text: Copy.providerAskedToWait(
            provider,
            formatTime(next, ctx.timeZone, ctx.now),
          ),
          style: LineStyle.Muted,
        });
      else if (lastAt && next) {
        const minutes = Math.max(
          1,
          Math.ceil(
            (timestampDate(next).getTime() - ctx.now.getTime()) / 60000,
          ),
        );
        lines.push({
          text: Copy.couldNotRefreshDetail(
            formatTime(lastAt, ctx.timeZone, ctx.now),
            minutes,
          ),
          style: LineStyle.Muted,
        });
      } else if (lastAt)
        lines.push({
          text: Copy.couldNotRefreshSince(
            formatTime(lastAt, ctx.timeZone, ctx.now),
          ),
          style: LineStyle.Muted,
        });
      return {
        ...base,
        state: WidgetState.Failed,
        primary: last?.text ? { text: last.text, stale: true } : undefined,
        usedPercent: undefined,
        badges: [{ tone: BadgeTone.Danger, label: Copy.couldNotRefresh }],
        lines,
        rows,
        updatedAt: date(lastAt),
      };
    }
  }
}

/** Partial aggregation details from the included devices' parser states. */
function partialDetails(ctx: ViewContext) {
  let skipped = 0n;
  let device: string | undefined;
  for (const entry of ctx.devices)
    for (const status of entry.parserStatuses)
      if (status.state === ParserState.PARTIAL) {
        skipped += status.skippedRecordCount;
        device ??= entry.name;
      }
  return { skipped: Number(skipped), device };
}

function usageView(
  widget: Widget,
  data: WidgetData,
  totals: UsageTotals | undefined,
  base: Omit<WidgetView, "accessibilityLabel" | "state">,
  ctx: ViewContext,
  extra: { rows?: ViewRow[]; bars?: number[]; cannotModel?: string } = {},
): Omit<WidgetView, "accessibilityLabel"> {
  const lines: ViewLine[] = [];
  const badges: ViewBadge[] = [];
  const cost = totals?.estimatedCost;
  let state = WidgetState.Value;
  let primary: WidgetView["primary"] = { text: formatTokens(totals?.tokens) };
  if (cost?.state === MetricState.VALUE && cost.amountUsd)
    lines.push({
      text: Copy.estimatedCost(formatDecimal(cost.amountUsd)),
      style: LineStyle.Muted,
    });
  else if (cost?.state === MetricState.PARTIAL && cost.amountUsd) {
    lines.push({
      text: Copy.estimatedCost(formatDecimal(cost.amountUsd)),
      style: LineStyle.Muted,
    });
    lines.push({
      text: `${Copy.cannotBeCalculated} for some usage. Token counts are still shown.`,
      style: LineStyle.Muted,
    });
  } else if (cost?.state === MetricState.CANNOT_BE_CALCULATED) {
    if (widget.type === WidgetType.LOCAL_USAGE_BREAKDOWN) {
      state = WidgetState.CannotBeCalculated;
      primary = { text: Copy.cannotBeCalculated, muted: true };
      if (extra.cannotModel)
        lines.push({
          text: Copy.cannotBeCalculatedDetail(extra.cannotModel),
          style: LineStyle.Muted,
          code: extra.cannotModel,
        });
    } else
      lines.push({
        text: `Est. cost: ${Copy.cannotBeCalculated}`,
        style: LineStyle.Muted,
      });
  }
  if (data.notices.includes(DataNotice.PARTIAL_AGGREGATION)) {
    const partial = partialDetails(ctx);
    badges.push({
      tone: BadgeTone.Warning,
      label: Copy.partialAggregation(partial.skipped),
    });
    if (partial.device)
      lines.push({
        text: Copy.partialAggregationDetail(partial.device),
        style: LineStyle.Muted,
        strong: partial.device,
        afterBadges: true,
      });
    if (state === WidgetState.Value) state = WidgetState.Partial;
  }
  if (data.notices.includes(DataNotice.NO_RUNNING_COLLECTOR))
    lines.push({
      text: "No collector is running right now. Values update when a device comes online.",
      style: LineStyle.Muted,
    });
  return {
    ...base,
    state,
    primary,
    badges,
    lines,
    rows: extra.rows ?? [],
    bars: extra.bars ?? [],
  };
}

const normalize = (values: bigint[]) => {
  const max = values.reduce((a, b) => (b > a ? b : a), 0n);
  return values.map((value) =>
    max === 0n ? 0 : Number((value * 1000n) / max) / 1000,
  );
};

/** Maps one widget and its latest data to the design-record presentation. */
export function widgetView(
  widget: Widget,
  data: WidgetData | undefined,
  ctx: ViewContext,
): WidgetView {
  const title = widgetTitle(widget, data);
  const base = {
    id: widget.id,
    title,
    badges: [] as ViewBadge[],
    lines: [] as ViewLine[],
    rows: [] as ViewRow[],
    bars: [] as number[],
    updatedAt: date(data?.summary?.dataUpdatedAt) ?? date(data?.generatedAt),
  };
  if (
    widget.sharingState === SharingState.ENDED ||
    data?.sharingState === SharingState.ENDED
  )
    return finish(
      {
        ...base,
        updatedAt: undefined,
        state: WidgetState.SharingEnded,
        badges: [{ tone: BadgeTone.Neutral, label: Copy.sharingEnded }],
        lines: [{ text: Copy.sharingEndedDetail, style: LineStyle.Muted }],
      },
      ctx,
    );
  if (!data) return finish({ ...base, state: WidgetState.Loading }, ctx);
  const content = data.content;
  switch (content.case) {
    case "metricPanel":
      return finish(
        metricPanelView(widget, data, content.value.readings, base, ctx),
        ctx,
      );
    case "usageSummary":
      return finish(
        usageView(widget, data, content.value.totals, base, ctx),
        ctx,
      );
    case "usageSeries": {
      const values = content.value.points.map((point) =>
        tokenTotal(point.totals?.tokens),
      );
      const total = values.reduce((a, b) => a + b, 0n);
      const view = usageView(
        widget,
        data,
        create(UsageTotalsSchema, { tokens: { inputTokens: total } }),
        base,
        ctx,
        { bars: normalize(values) },
      );
      return finish(view, ctx);
    }
    case "usageBreakdown": {
      const breakdown = content.value;
      const cannot = breakdown.rows.find(
        (row) =>
          row.totals?.estimatedCost?.state === MetricState.CANNOT_BE_CALCULATED,
      );
      const rows = breakdown.rows.map((row) => ({
        label: row.model || row.deviceName || toolName(row.tool),
        value:
          row.totals?.estimatedCost?.state === MetricState.VALUE
            ? `${formatTokens(row.totals.tokens)} · $${formatDecimal(row.totals.estimatedCost.amountUsd)}`
            : formatTokens(row.totals?.tokens),
      }));
      return finish(
        usageView(widget, data, breakdown.total, base, ctx, {
          rows,
          cannotModel: cannot?.model,
        }),
        ctx,
      );
    }
    case "metricHistory": {
      const series = content.value.series[0];
      const points = series?.points ?? [];
      const latest = [...points]
        .reverse()
        .find((point) => point.value?.state === MetricState.VALUE);
      const numeric = points.map((point) =>
        point.value?.value.case === "percent"
          ? BigInt(Math.round(point.value.value.value * 100))
          : point.value?.value.case === "integer"
            ? point.value.value.value
            : point.value?.value.case === "money"
              ? BigInt(Math.round(Number(point.value.value.value.amount) * 100))
              : 0n,
      );
      if (!latest || !series)
        return finish(
          {
            ...base,
            state: WidgetState.NotProvided,
            primary: { text: Copy.notProvided, muted: true },
          },
          ctx,
        );
      return finish(
        {
          ...base,
          state: WidgetState.Value,
          primary: {
            text: formatMetricValue(
              latest.value,
              series.kind,
              series.unit,
              ctx.timeZone,
              ctx.now,
            ),
          },
          lines: [
            { text: dimensionLabel(series.dimension), style: LineStyle.Muted },
          ],
          bars: normalize(numeric),
          updatedAt: date(latest.observedAt),
        },
        ctx,
      );
    }
    case "resourceList": {
      const resources = content.value.resources;
      const ready = resources.filter(
        (resource) => resource.health === ResourceHealth.HEALTHY,
      ).length;
      const failing = resources.filter(
        (resource) =>
          resource.health === ResourceHealth.FAILED ||
          resource.health === ResourceHealth.DEGRADED,
      ).length;
      return finish(
        {
          ...base,
          state: WidgetState.Value,
          primary: {
            text: `${resources.length} ${resources.length === 1 ? "project" : "projects"}`,
          },
          badges: [
            ...(ready
              ? [{ tone: BadgeTone.Success, label: Copy.projectsReady(ready) }]
              : []),
            ...(failing
              ? [{ tone: BadgeTone.Danger, label: `${failing} need attention` }]
              : []),
          ],
          rows: resources.map((resource) => ({
            label: resource.displayName || resource.resourceId,
            value:
              resource.statusState === MetricState.NOT_PROVIDED
                ? Copy.notProvided
                : resource.providerStatus ||
                  ResourceHealth[resource.health].toLowerCase(),
            muted: resource.statusState !== MetricState.VALUE,
          })),
        },
        ctx,
      );
    }
    default:
      return finish(
        {
          ...base,
          state: WidgetState.NotProvided,
          primary: { text: Copy.notProvided, muted: true },
        },
        ctx,
      );
  }
}

/** Devices whose usage a dashboard combines, for the duplicate notice. */
export function duplicateDeviceCount(
  data: WidgetData[],
  devices: Device[],
): number | undefined {
  if (
    !data.some((entry) =>
      entry.notices.includes(DataNotice.POSSIBLE_CROSS_DEVICE_DUPLICATION),
    )
  )
    return undefined;
  const named = new Set<string>();
  for (const entry of data)
    if (entry.content.case === "usageBreakdown")
      for (const row of entry.content.value.rows)
        if (row.deviceId) named.add(row.deviceId);
  return Math.max(2, named.size || devices.length);
}
