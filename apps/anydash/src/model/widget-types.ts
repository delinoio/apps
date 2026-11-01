// SPDX-License-Identifier: Apache-2.0
import { pick } from "../format";
import {
  Provider,
  QueryPeriod,
  TimeGranularity,
  UsageBreakdownDimension,
  WidgetPresentation,
  WidgetType,
} from "@repo/protos/anydash/v1/anydash_pb";

export enum SourceKind {
  Subscription = "subscription",
  Deployment = "deployment",
  AnyConnection = "any-connection",
  Devices = "devices",
}

export interface WidgetTypeSpec {
  type: WidgetType;
  label: string;
  presentations: WidgetPresentation[];
  source: SourceKind;
  period: boolean;
  granularity: boolean;
  breakdown: boolean;
}

/** Allowed presentations, sources and query fields per the protocol table. */
export const widgetTypes: WidgetTypeSpec[] = [
  {
    type: WidgetType.SUBSCRIPTION_LIMITS,
    label: "Subscription limits",
    presentations: [
      WidgetPresentation.GAUGE,
      WidgetPresentation.NUMBER,
      WidgetPresentation.LIST,
    ],
    source: SourceKind.Subscription,
    period: false,
    granularity: false,
    breakdown: false,
  },
  {
    type: WidgetType.SUBSCRIPTION_HISTORY,
    label: "Subscription history",
    presentations: [
      WidgetPresentation.LINE_CHART,
      WidgetPresentation.BAR_CHART,
    ],
    source: SourceKind.Subscription,
    period: true,
    granularity: true,
    breakdown: false,
  },
  {
    type: WidgetType.LOCAL_USAGE_SUMMARY,
    label: "Local usage",
    presentations: [WidgetPresentation.NUMBER, WidgetPresentation.TABLE],
    source: SourceKind.Devices,
    period: true,
    granularity: false,
    breakdown: false,
  },
  {
    type: WidgetType.LOCAL_USAGE_SERIES,
    label: "Local usage over time",
    presentations: [
      WidgetPresentation.BAR_CHART,
      WidgetPresentation.LINE_CHART,
    ],
    source: SourceKind.Devices,
    period: true,
    granularity: true,
    breakdown: false,
  },
  {
    type: WidgetType.LOCAL_USAGE_BREAKDOWN,
    label: "Local usage breakdown",
    presentations: [
      WidgetPresentation.TABLE,
      WidgetPresentation.BAR_CHART,
      WidgetPresentation.LIST,
    ],
    source: SourceKind.Devices,
    period: true,
    granularity: false,
    breakdown: true,
  },
  {
    type: WidgetType.DEPLOYMENT_RESOURCES,
    label: "Deployments",
    presentations: [WidgetPresentation.LIST, WidgetPresentation.TABLE],
    source: SourceKind.Deployment,
    period: false,
    granularity: false,
    breakdown: false,
  },
  {
    type: WidgetType.DEPLOYMENT_USAGE,
    label: "Deployment usage",
    presentations: [
      WidgetPresentation.NUMBER,
      WidgetPresentation.TABLE,
      WidgetPresentation.BAR_CHART,
    ],
    source: SourceKind.Deployment,
    period: false,
    granularity: false,
    breakdown: false,
  },
  {
    type: WidgetType.DEPLOYMENT_COST,
    label: "Deployment cost",
    presentations: [WidgetPresentation.NUMBER, WidgetPresentation.TABLE],
    source: SourceKind.Deployment,
    period: false,
    granularity: false,
    breakdown: false,
  },
  {
    type: WidgetType.METRIC_HISTORY,
    label: "Metric history",
    presentations: [
      WidgetPresentation.LINE_CHART,
      WidgetPresentation.BAR_CHART,
    ],
    source: SourceKind.AnyConnection,
    period: true,
    granularity: true,
    breakdown: false,
  },
];

export const specFor = (type: WidgetType) =>
  widgetTypes.find((spec) => spec.type === type) ?? widgetTypes[0];

export const presentationLabel = (presentation: WidgetPresentation) =>
  pick(
    {
      [WidgetPresentation.NUMBER]: "Number",
      [WidgetPresentation.GAUGE]: "Gauge",
      [WidgetPresentation.LINE_CHART]: "Line chart",
      [WidgetPresentation.BAR_CHART]: "Bar chart",
      [WidgetPresentation.TABLE]: "Table",
      [WidgetPresentation.LIST]: "List",
    },
    presentation,
    "Number",
  );

export const periodLabel = (period: QueryPeriod) =>
  pick(
    {
      [QueryPeriod.LAST_24_HOURS]: "Last 24 hours",
      [QueryPeriod.LAST_7_DAYS]: "Last 7 days",
      [QueryPeriod.LAST_30_DAYS]: "Last 30 days",
      [QueryPeriod.LAST_90_DAYS]: "Last 90 days",
      [QueryPeriod.TODAY]: "Today",
      [QueryPeriod.THIS_WEEK]: "This week",
      [QueryPeriod.THIS_MONTH]: "This month",
    },
    period,
    "Last 7 days",
  );

export const periods = [
  QueryPeriod.TODAY,
  QueryPeriod.LAST_24_HOURS,
  QueryPeriod.LAST_7_DAYS,
  QueryPeriod.THIS_WEEK,
  QueryPeriod.THIS_MONTH,
  QueryPeriod.LAST_30_DAYS,
  QueryPeriod.LAST_90_DAYS,
];

/** HOUR granularity is valid only for the shorter periods. */
export const granularityFor = (period: QueryPeriod) =>
  [
    QueryPeriod.LAST_24_HOURS,
    QueryPeriod.TODAY,
    QueryPeriod.LAST_7_DAYS,
    QueryPeriod.THIS_WEEK,
  ].includes(period)
    ? TimeGranularity.HOUR
    : TimeGranularity.DAY;

export const breakdownLabel = (dimension: UsageBreakdownDimension) =>
  pick(
    {
      [UsageBreakdownDimension.DEVICE]: "By device",
      [UsageBreakdownDimension.TOOL]: "By tool",
      [UsageBreakdownDimension.MODEL]: "By model",
    },
    dimension,
    "By model",
  );

const subscription = [Provider.CODEX, Provider.CLAUDE_CODE, Provider.GROK];
const deployment = [Provider.VERCEL, Provider.SUPABASE, Provider.CLOUDFLARE];
export const acceptsProvider = (source: SourceKind, provider: Provider) =>
  source === SourceKind.AnyConnection ||
  (source === SourceKind.Subscription && subscription.includes(provider)) ||
  (source === SourceKind.Deployment && deployment.includes(provider));
