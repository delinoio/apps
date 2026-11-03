// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "@rstest/core";
import { create, toBinary } from "@bufbuild/protobuf";
import { base64Encode } from "@bufbuild/protobuf/wire";
import { durationFromMs, timestampFromDate } from "@bufbuild/protobuf/wkt";
import {
  Code,
  ConnectError,
  createClient,
  createRouterTransport,
} from "@connectrpc/connect";
import {
  AccountService,
  ClientKind,
  ErrorDetailSchema,
  ErrorReason,
  GetAccountResponseSchema,
  LocalCollectorService,
  NativeWidgetSnapshotSchema,
  QuotaKind,
  RateLimitScope,
} from "@repo/protos/anydash/v1/anydash_pb";
import { Copy } from "../src/copy";
import {
  ProblemKind,
  quotaThings,
  shouldRetry,
  toProblem,
} from "../src/errors";
import {
  formatCompact,
  formatDecimal,
  formatMoney,
  formatTime,
} from "../src/format";
import {
  clientKind,
  clientKindHeader,
  HostPlatform,
  RpcTarget,
  type RpcRequest,
} from "../src/host";
import {
  applyKey,
  clampBox,
  describeBox,
  EditKey,
  keepDraftPositions,
  layoutMode,
  LayoutMode,
  placeNew,
  resolveCollisions,
  type Box,
} from "../src/model/grid";
import {
  nextAllowedFrom,
  refreshTargets,
  remainingSeconds,
} from "../src/model/refresh";
import { MAX_SNAPSHOTS, pruneSnapshots } from "../src/model/snapshots";
import {
  parseRoute,
  routePath,
  RouteKind,
  widgetDeepLink,
} from "../src/model/routes";
import { createHostTransport } from "../src/transport";
import * as fixtures from "./fixtures/scenarios";

describe("grid", () => {
  const boxes: Box[] = [
    { id: "a", x: 0, y: 0, width: 4, height: 2 },
    { id: "b", x: 4, y: 0, width: 4, height: 2 },
    { id: "c", x: 8, y: 0, width: 4, height: 2 },
  ];
  it("chooses 3, 2 and 1 across by width", () => {
    expect(layoutMode(1280)).toBe(LayoutMode.Wide);
    expect(layoutMode(1279)).toBe(LayoutMode.Tablet);
    expect(layoutMode(768)).toBe(LayoutMode.Tablet);
    expect(layoutMode(767)).toBe(LayoutMode.Phone);
  });
  it("clamps to the protocol layout rules", () => {
    expect(clampBox({ id: "a", x: 11, y: -3, width: 20, height: 40 })).toEqual({
      id: "a",
      x: 0,
      y: 0,
      width: 12,
      height: 12,
    });
    expect(clampBox({ id: "a", x: 11, y: 0, width: 4, height: 2 }).x).toBe(8);
  });
  it("moves with arrows and resizes with Shift, pushing overlaps down", () => {
    const moved = applyKey(boxes, "a", EditKey.Right, false);
    expect(moved.find((box) => box.id === "a")).toMatchObject({ x: 1, y: 0 });
    expect(moved.find((box) => box.id === "b")).toMatchObject({ x: 4, y: 2 });
    const resized = applyKey(boxes, "c", EditKey.Down, true);
    expect(resized.find((box) => box.id === "c")).toMatchObject({ height: 3 });
    const narrowed = applyKey(boxes, "a", EditKey.Left, true);
    expect(narrowed.find((box) => box.id === "a")).toMatchObject({ width: 3 });
    expect(applyKey(boxes, "a", EditKey.Up, false)).toEqual(
      resolveCollisions(boxes, "a"),
    );
  });
  it("places new widgets in the first free cell", () => {
    expect(placeNew(boxes, "d")).toEqual({
      id: "d",
      x: 0,
      y: 2,
      width: 4,
      height: 2,
    });
    expect(placeNew(boxes.slice(0, 2), "d")).toMatchObject({ x: 8, y: 0 });
  });
  it("keeps draft positions when a widget is added or removed while editing", () => {
    const draft = [{ ...boxes[0], x: 4, y: 2 }, boxes[1], boxes[2]];
    const added = { id: "d", x: 0, y: 0, width: 4, height: 2 };
    expect(keepDraftPositions(draft, [...boxes, added])).toEqual([
      ...draft,
      added,
    ]);
    expect(keepDraftPositions(draft, [boxes[0], boxes[2]])).toEqual([
      draft[0],
      draft[2],
    ]);
  });
  it("announces position and size", () => {
    expect(
      describeBox("Codex", { id: "a", x: 4, y: 1, width: 1, height: 3 }),
    ).toBe("Codex: column 5, row 2, 1 column wide, 3 rows tall.");
  });
});

describe("routes and deep links", () => {
  const dashboard = `/spaces/${fixtures.ids.personal}/dashboards/${fixtures.ids.overview}`;
  it("parses web routes and native deep links", () => {
    expect(parseRoute("/")).toEqual({ kind: RouteKind.Home });
    expect(parseRoute("/settings")).toEqual({ kind: RouteKind.Settings });
    expect(parseRoute(`/connect/${fixtures.ids.enrollment}`)).toEqual({
      kind: RouteKind.Connect,
      enrollmentId: fixtures.ids.enrollment,
    });
    const link = widgetDeepLink(
      fixtures.ids.personal,
      fixtures.ids.overview,
      fixtures.ids.widget(1),
    );
    expect(link).toBe(`anydash:/${dashboard}?widget=${fixtures.ids.widget(1)}`);
    expect(parseRoute(link)).toEqual({
      kind: RouteKind.Dashboard,
      spaceId: fixtures.ids.personal,
      dashboardId: fixtures.ids.overview,
      widgetId: fixtures.ids.widget(1),
    });
    expect(
      parseRoute(`anydash://connect/${fixtures.ids.enrollment}`).kind,
    ).toBe(RouteKind.Connect);
    expect(
      routePath(parseRoute(`${dashboard}?widget=${fixtures.ids.widget(1)}`)),
    ).toBe(`${dashboard}?widget=${fixtures.ids.widget(1)}`);
  });
  it("rejects malformed identifiers", () => {
    expect(parseRoute("/spaces/x/dashboards/y").kind).toBe(RouteKind.NotFound);
    expect("widgetId" in parseRoute(`${dashboard}?widget=bad`)).toBe(false);
  });
});

describe("transport", () => {
  it("sends client kind and version headers and decodes responses", async () => {
    const sent: RpcRequest[] = [];
    const transport = createHostTransport(
      RpcTarget.Api,
      () => ClientKind.DESKTOP,
      async (request) => {
        sent.push(request);
        return {
          status: 200,
          body: base64Encode(
            toBinary(
              GetAccountResponseSchema,
              create(GetAccountResponseSchema, { account: fixtures.account() }),
            ),
          ),
        };
      },
    );
    const response = await createClient(AccountService, transport).getAccount(
      {},
    );
    expect(response.account?.timeZone).toBe("Asia/Seoul");
    expect(sent[0]).toMatchObject({
      target: RpcTarget.Api,
      service: "anydash.v1.AccountService",
      method: "GetAccount",
      headers: {
        "X-AnyDash-Client-Kind": "CLIENT_KIND_DESKTOP",
        "X-AnyDash-Client-Version": "0.1.0-test",
      },
    });
  });
  it("maps platforms to client kinds", () => {
    expect(clientKindHeader(clientKind(HostPlatform.Web))).toBe(
      "CLIENT_KIND_WEB",
    );
    expect(clientKindHeader(clientKind(HostPlatform.Ios))).toBe(
      "CLIENT_KIND_IOS",
    );
    expect(clientKindHeader(clientKind(HostPlatform.Android))).toBe(
      "CLIENT_KIND_ANDROID",
    );
  });
  it.each([
    [
      ErrorReason.UPDATE_REQUIRED,
      "failed_precondition",
      400,
      ProblemKind.UpdateRequired,
    ],
    [ErrorReason.VERSION_CONFLICT, "aborted", 409, ProblemKind.VersionConflict],
    [
      ErrorReason.QUOTA_EXCEEDED,
      "resource_exhausted",
      429,
      ProblemKind.QuotaExceeded,
    ],
    [
      ErrorReason.RATE_LIMITED,
      "resource_exhausted",
      429,
      ProblemKind.RateLimited,
    ],
    [
      ErrorReason.SPACE_INACTIVE,
      "permission_denied",
      403,
      ProblemKind.SpaceInactive,
    ],
  ])(
    "classifies %s errors with their details",
    async (reason, code, status, kind) => {
      const detail = create(ErrorDetailSchema, {
        reason,
        correlationId: "0192f9aa-0000-7000-8000-000000000001",
        retryAfter: durationFromMs(41_500),
        rateLimitScope: RateLimitScope.MANUAL_REFRESH,
        quota: { kind: QuotaKind.WIDGETS_PER_DASHBOARD, limit: 100, used: 100 },
        currentVersion: 9n,
      });
      const body = JSON.stringify({
        code,
        message: "rejected",
        details: [
          {
            type: ErrorDetailSchema.typeName,
            value: base64Encode(toBinary(ErrorDetailSchema, detail)),
          },
        ],
      });
      const transport = createHostTransport(
        RpcTarget.Api,
        () => ClientKind.WEB,
        async () => ({
          status,
          body: base64Encode(new TextEncoder().encode(body)),
          correlationId: "0192f9aa-0000-7000-8000-000000000001",
        }),
      );
      const error = await createClient(AccountService, transport)
        .getAccount({})
        .catch((e) => e);
      const problem = toProblem(error);
      expect(problem.kind).toBe(kind);
      expect(problem.correlationId).toBe(
        "0192f9aa-0000-7000-8000-000000000001",
      );
      expect(problem.retryAfterSeconds).toBe(42);
      expect(problem.quotaLimit).toBe(100);
      expect(problem.currentVersion).toBe("9");
      // Quota, update, conflict and rate-limit errors never retry automatically.
      expect(shouldRetry(0, error)).toBe(false);
    },
  );
  it("names quota things in the limit guidance", () => {
    expect(
      Copy.limitReachedDetail(50, quotaThings(QuotaKind.DASHBOARDS_PER_SPACE)),
    ).toBe(
      "You can have up to 50 dashboards in a space. Delete one to add another.",
    );
  });
});

describe("refresh limits", () => {
  it("targets only the caller's sources", () => {
    const targets = refreshTargets(
      fixtures.makeDashboard(),
      fixtures.devices(),
    );
    expect(targets.connectionIds).toContain(fixtures.ids.codex);
    expect(targets.connectionIds).not.toContain(fixtures.ids.supabase);
    expect(targets.deviceIds).toEqual([
      fixtures.ids.studio,
      fixtures.ids.buildBox,
      fixtures.ids.laptop,
    ]);
  });
  it("waits at least one minute and honours server waits", () => {
    const now = 1_000_000;
    expect(nextAllowedFrom(now, [])).toBe(now + 60_000);
    expect(nextAllowedFrom(now, [{ retryAfterSeconds: 90 }])).toBe(
      now + 90_000,
    );
    expect(
      nextAllowedFrom(now, [
        { nextAllowedAt: timestampFromDate(new Date(now + 70_000)) },
      ]),
    ).toBe(now + 70_000);
    expect(remainingSeconds(now + 41_200, now)).toBe(42);
    expect(remainingSeconds(now - 1, now)).toBe(0);
  });
});

describe("formatting", () => {
  it("formats values without converting currencies", () => {
    expect(formatMoney({ currencyCode: "USD", amount: "41.8" } as never)).toBe(
      "$41.80",
    );
    expect(
      formatMoney({ currencyCode: "EUR", amount: "1234.567" } as never),
    ).toBe("1,234.57 EUR");
    expect(formatDecimal("214.300000")).toBe("214.30");
    expect(formatDecimal("0.995")).toBe("1.00");
    expect(formatCompact(48_200_000n)).toBe("48.2M");
    expect(formatCompact(950)).toBe("950");
  });
  it("formats times in the selected time zone", () => {
    expect(formatTime(fixtures.NOW, "Asia/Seoul", fixtures.NOW)).toBe("12:41");
    expect(
      formatTime(new Date("2026-10-31T09:05:00Z"), "Asia/Seoul", fixtures.NOW),
    ).toBe("Oct 31, 18:05");
    expect(formatTime(fixtures.NOW, "UTC", fixtures.NOW)).toBe("03:41");
  });
});

describe("native widget snapshots", () => {
  it("prunes more stored snapshots than one write accepts in ordered batches", async () => {
    const stored = Array.from({ length: 130 }, (_, index) =>
      create(NativeWidgetSnapshotSchema, {
        dashboardId: index < 10 ? "deleted" : "kept",
        widgetId: `widget-${index}`,
      }),
    );
    const writes: { ids: string[]; replaceAll: boolean }[] = [];
    const collector = createRouterTransport(({ service }) => {
      service(LocalCollectorService, {
        listNativeWidgetSnapshots: () => ({ snapshots: stored }),
        putNativeWidgetSnapshots: (request) => {
          if (request.snapshots.length > MAX_SNAPSHOTS)
            throw new ConnectError("too many", Code.InvalidArgument);
          writes.push({
            ids: request.snapshots.map((snapshot) => snapshot.widgetId),
            replaceAll: request.replaceAll,
          });
          return {};
        },
      });
    });
    await pruneSnapshots(
      HostPlatform.Desktop,
      collector,
      (snapshot) => snapshot.dashboardId === "deleted",
    );
    expect(writes.map((write) => [write.ids.length, write.replaceAll])).toEqual(
      [
        [100, true],
        [20, false],
      ],
    );
    expect(writes.flatMap((write) => write.ids)).toEqual(
      stored.slice(10).map((snapshot) => snapshot.widgetId),
    );
  });
});
