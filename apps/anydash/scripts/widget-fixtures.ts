// SPDX-License-Identifier: Apache-2.0
// Binary fixtures for the native widget decoders (Swift, Kotlin and C#),
// produced by the TypeScript bindings. scripts/widget-fixtures.test.mjs checks
// that the committed files match this generator.
import { create, toBinary } from "@bufbuild/protobuf";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import {
  ListNativeWidgetSnapshotsResponseSchema,
  MetricState,
  QueryWidgetDataResponseSchema,
  SharingState,
  WidgetType,
} from "../../../protos/anydash/v1/anydash_pb.ts";

const at = (iso: string) => timestampFromDate(new Date(iso));
export function fixtures() {
  const snapshots = toBinary(
    ListNativeWidgetSnapshotsResponseSchema,
    create(ListNativeWidgetSnapshotsResponseSchema, {
      snapshots: [
        {
          dashboardId: "0192f000-0000-7000-8000-0000000000c1",
          widgetId: "0192f4aa-0000-7000-8000-000000000001",
          spaceId: "0192f000-0000-7000-8000-0000000000a1",
          sharingState: SharingState.PRIVATE,
          refreshedAt: at("2026-11-01T03:41:00Z"),
          summary: {
            title: "Codex · 5-hour",
            primaryLabel: "left",
            primaryValue: {
              state: MetricState.VALUE,
              value: { case: "percent", value: 62 },
            },
            secondaryLabel: "Resets",
            secondaryValue: {
              state: MetricState.VALUE,
              value: { case: "time", value: at("2026-11-01T05:20:00Z") },
            },
            dataUpdatedAt: at("2026-11-01T03:41:00Z"),
          },
        },
        {
          dashboardId: "0192f000-0000-7000-8000-0000000000c2",
          widgetId: "0192f4aa-0000-7000-8000-000000000008",
          spaceId: "0192f000-0000-7000-8000-0000000000b1",
          sharingState: SharingState.ENDED,
          refreshedAt: at("2026-11-01T03:41:00Z"),
        },
        {
          dashboardId: "0192f000-0000-7000-8000-0000000000c1",
          widgetId: "0192f4aa-0000-7000-8000-000000000007",
          spaceId: "0192f000-0000-7000-8000-0000000000a1",
          sharingState: SharingState.PRIVATE,
          refreshedAt: at("2026-11-01T03:41:00Z"),
          summary: {
            title: "Vercel · Month to date",
            primaryLabel: "billed",
            primaryValue: {
              state: MetricState.VALUE,
              value: {
                case: "money",
                value: { currencyCode: "USD", amount: "41.8" },
              },
            },
            dataUpdatedAt: at("2026-11-01T03:41:00Z"),
          },
        },
      ],
    }),
  );
  const widgetData = toBinary(
    QueryWidgetDataResponseSchema,
    create(QueryWidgetDataResponseSchema, {
      data: {
        widgetId: "0192f4aa-0000-7000-8000-000000000001",
        type: WidgetType.SUBSCRIPTION_LIMITS,
        sharingState: SharingState.PRIVATE,
        generatedAt: at("2026-11-01T03:41:00Z"),
        summary: {
          title: "Codex · 5-hour",
          primaryLabel: "left",
          primaryValue: {
            state: MetricState.VALUE,
            value: { case: "percent", value: 61 },
          },
        },
        content: { case: "metricPanel", value: { readings: [] } },
      },
    }),
  );
  return { "snapshots.pb": snapshots, "widget-data.pb": widgetData };
}

if (process.argv[2] === "--write") {
  const { writeFileSync } = await import("node:fs");
  for (const [name, bytes] of Object.entries(fixtures()))
    writeFileSync(
      new URL(`../native/widgets/fixtures/${name}`, import.meta.url),
      bytes,
    );
}
