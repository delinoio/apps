// SPDX-License-Identifier: Apache-2.0
import { useEffect, useMemo, useRef, useState } from "@lynx-js/react";
import type { KeyEvent } from "@lynx-js/types";
import { timestampDate } from "@bufbuild/protobuf/wkt";
import { createClient } from "@connectrpc/connect";
import {
  createConnectQueryKey,
  useMutation,
  useQuery,
} from "@connectrpc/connect-query";
import { useQueryClient } from "@tanstack/react-query";
import { Button, Variant } from "@packages/lynx-ui";
import {
  ConnectionService,
  DashboardService,
  DeviceService,
  LocalCollectorService,
  SharingState,
  SpaceKind,
  SpacePermission,
  WidgetDataService,
  type Dashboard as DashboardMessage,
  type ListDashboardsResponse,
  type QueryDashboardDataResponse,
  type Space,
  type Widget,
  type WidgetData,
} from "@repo/protos/anydash/v1/anydash_pb";
import {
  AddWidgetDialog,
  queryFor,
  type NewWidget,
} from "../components/AddWidgetDialog";
import { Modal } from "../components/dialog";
import { Grid } from "../components/Grid";
import { A11y, Role, SmallButton, T } from "../components/primitives";
import {
  AccessDeniedState,
  DuplicateNotice,
  ErrorState,
  LimitReached,
  LoadingState,
  RecoveryState,
  SpaceUnavailableState,
} from "../components/States";
import { WidgetCard } from "../components/WidgetCard";
import { Copy, ExtraCopy } from "../copy";
import { ProblemKind, toProblem, type Problem } from "../errors";
import { hostCall, HostMethod, HostPlatform } from "../host";
import {
  endedWidgetIds,
  purgeWidgets,
  readCachedWidgets,
  writeCachedWidgets,
} from "../model/cache";
import {
  applyKey,
  changedBoxes,
  describeBox,
  EditKey,
  isEditKey,
  keepDraftPositions,
  LayoutMode,
  placeNew,
  type Box,
} from "../model/grid";
import {
  AUTO_REFRESH_MS,
  nextAllowedFrom,
  refreshTargets,
  remainingSeconds,
  serverNextAllowed,
} from "../model/refresh";
import { RouteKind } from "../model/routes";
import {
  buildSnapshots,
  pruneSnapshots,
  publishSnapshots,
} from "../model/snapshots";
import {
  duplicateDeviceCount,
  widgetView,
  type ViewContext,
} from "../model/widget-view";
import { useNavigation } from "../navigation";
import { useSession } from "../session";
import { uuidv7 } from "../uuid";
import { useNow, useVisible } from "../visibility";

const boxesOf = (dashboard: DashboardMessage | undefined): Box[] =>
  (dashboard?.widgets ?? []).map((widget) => ({
    id: widget.id,
    x: widget.layout?.x ?? 0,
    y: widget.layout?.y ?? 0,
    width: widget.layout?.width || 4,
    height: widget.layout?.height || 2,
  }));

export const can = (space: Space | undefined, permission: SpacePermission) =>
  !!space?.permissions.includes(permission);

/** Removes widgets whose sharing ended from every local copy, then shows the state. */
export function stripEnded(
  response: QueryDashboardDataResponse | undefined,
  ended: string[],
) {
  if (!response) return response;
  const stale = response.widgets.some(
    (entry) =>
      ended.includes(entry.widgetId) &&
      (entry.sharingState !== SharingState.ENDED ||
        entry.summary !== undefined ||
        entry.notices.length > 0 ||
        entry.content.case !== undefined),
  );
  // Unchanged responses keep their identity so callers can skip cache writes.
  if (!stale) return response;
  return {
    ...response,
    widgets: response.widgets.map((entry) =>
      ended.includes(entry.widgetId)
        ? {
            ...entry,
            sharingState: SharingState.ENDED,
            summary: undefined,
            notices: [],
            content: { case: undefined },
          }
        : entry,
    ),
  } as QueryDashboardDataResponse;
}

export function DashboardScreen({
  space,
  dashboardId,
  focusWidgetId,
  mode,
  onRefreshState,
  editing,
  setEditing,
}: {
  space: Space;
  dashboardId: string;
  focusWidgetId?: string;
  mode: LayoutMode;
  onRefreshState: (state: {
    updatedAt?: Date;
    remaining: number;
    refresh: () => void;
    busy: boolean;
  }) => void;
  editing: boolean;
  setEditing: (value: boolean) => void;
}) {
  const session = useSession();
  const { navigate } = useNavigation();
  const queryClient = useQueryClient();
  const visible = useVisible();
  const timeZone = session.account?.timeZone || "UTC";
  const dashboardKey = createConnectQueryKey({
    schema: DashboardService.method.getDashboard,
    input: { dashboardId },
    transport: session.transport,
    cardinality: "finite",
  });
  const dataKey = createConnectQueryKey({
    schema: WidgetDataService.method.queryDashboardData,
    input: { dashboardId, timeZone },
    transport: session.transport,
    cardinality: "finite",
  });
  const blocked = session.updateRequired;
  const dashboardQuery = useQuery(
    DashboardService.method.getDashboard,
    { dashboardId },
    { enabled: !blocked },
  );
  const dashboard = dashboardQuery.data?.dashboard;
  const dataQuery = useQuery(
    WidgetDataService.method.queryDashboardData,
    { dashboardId, timeZone },
    {
      enabled: !!dashboard && !blocked,
      refetchInterval: visible ? AUTO_REFRESH_MS : false,
      refetchIntervalInBackground: false,
    },
  );
  const devicesQuery = useQuery(
    DeviceService.method.listDevices,
    { page: { size: 200 } },
    { enabled: !blocked },
  );
  const connectionsQuery = useQuery(
    ConnectionService.method.listConnections,
    { page: { size: 200 } },
    { enabled: !blocked },
  );
  const devices = devicesQuery.data?.devices ?? [];
  const connections = connectionsQuery.data?.connections ?? [];

  // Cached data from the last session shows while offline until the first
  // response; it never includes widgets whose sharing ended.
  const [cached, setCached] = useState<Map<string, WidgetData>>(new Map());
  const widgetIds = (dashboard?.widgets ?? [])
    .map((widget) => widget.id)
    .join(",");
  useEffect(() => {
    if (!widgetIds) return;
    let active = true;
    void readCachedWidgets(session.userKey, widgetIds.split(",")).then(
      (found) => active && setCached(found),
    );
    return () => {
      active = false;
    };
  }, [widgetIds, session.userKey]);

  const ended = useMemo(
    () => endedWidgetIds(dashboard, dataQuery.data?.widgets),
    [dashboard, dataQuery.data],
  );
  useEffect(() => {
    if (!dashboard || !dataQuery.data) return;
    if (ended.length) {
      void purgeWidgets(session.userKey, ended);
      setCached((previous) => {
        const next = new Map(previous);
        for (const id of ended) next.delete(id);
        return next;
      });
      // setQueryData always bumps dataUpdatedAt, which re-runs this effect;
      // write only when ended data remains, or the purge never settles.
      const stripped = stripEnded(dataQuery.data, ended);
      if (stripped !== dataQuery.data)
        queryClient.setQueryData(dataKey, stripped);
    }
    void writeCachedWidgets(session.userKey, dataQuery.data.widgets);
    // Drop the native snapshots of widgets no longer on this dashboard, such
    // as one removed here or on another device, before publishing the rest.
    const live = new Set(dashboard.widgets.map((widget) => widget.id));
    void pruneSnapshots(
      session.info.platform,
      session.collector,
      (snapshot) =>
        snapshot.dashboardId === dashboard.id && !live.has(snapshot.widgetId),
    );
    void publishSnapshots(
      session.info.platform,
      buildSnapshots(dashboard, dataQuery.data.widgets),
      session.collector,
    );
    if (dataQuery.data.dashboardVersion > dashboard.version && !editing)
      void queryClient.invalidateQueries({ queryKey: dashboardKey });
  }, [dataQuery.dataUpdatedAt, dashboard?.version]);

  const dataById = useMemo(() => {
    const map = new Map<string, WidgetData>();
    for (const [id, entry] of cached)
      if (!ended.includes(id)) map.set(id, entry);
    for (const entry of dataQuery.data?.widgets ?? [])
      map.set(entry.widgetId, entry);
    return map;
  }, [cached, dataQuery.data, ended]);

  // Collector timing for failure guidance is available on desktop only.
  const [nextAttempt, setNextAttempt] = useState<ViewContext["nextAttempt"]>();
  useEffect(() => {
    if (!session.collector) return;
    void createClient(LocalCollectorService, session.collector)
      .getCollectorStatus({})
      .then((response) =>
        setNextAttempt(
          new Map(
            (response.status?.connections ?? []).map((entry) => [
              entry.connectionId,
              entry.nextAttemptAt,
            ]),
          ),
        ),
      )
      .catch(() => undefined);
  }, [dataQuery.dataUpdatedAt]);

  // Manual refresh: once per minute per connection or device, with the wait shown.
  const [nextAllowed, setNextAllowed] = useState(0);
  const [refreshing, setRefreshing] = useState(false);
  const targets = refreshTargets(dashboard, devices);
  const deadline = Math.max(
    nextAllowed,
    serverNextAllowed(targets, connections),
  );
  const now = useNow(1000, deadline > Date.now());
  const remaining = remainingSeconds(deadline, now);
  const refresh = async () => {
    if (remaining > 0 || refreshing) return;
    setRefreshing(true);
    const clients = {
      connections: createClient(ConnectionService, session.transport),
      devices: createClient(DeviceService, session.transport),
    };
    const results = await Promise.all([
      ...targets.connectionIds.map((connectionId) =>
        clients.connections
          .requestConnectionRefresh({ connectionId })
          .then((response) => ({ nextAllowedAt: response.nextAllowedAt }))
          .catch((error) => ({
            retryAfterSeconds: toProblem(error).retryAfterSeconds,
          })),
      ),
      ...targets.deviceIds.map((deviceId) =>
        clients.devices
          .requestDeviceSync({ deviceId })
          .then((response) => ({ nextAllowedAt: response.nextAllowedAt }))
          .catch((error) => ({
            retryAfterSeconds: toProblem(error).retryAfterSeconds,
          })),
      ),
    ]);
    setNextAllowed(nextAllowedFrom(Date.now(), results));
    await dataQuery.refetch();
    setRefreshing(false);
  };
  const updatedAt = (dataQuery.data?.widgets ?? []).reduce<Date | undefined>(
    (latest, entry) => {
      const value = entry.generatedAt
        ? timestampDate(entry.generatedAt)
        : undefined;
      return value && (!latest || value > latest) ? value : latest;
    },
    undefined,
  );
  const refreshState = useRef(onRefreshState);
  refreshState.current = onRefreshState;
  useEffect(() => {
    refreshState.current({
      updatedAt,
      remaining,
      refresh: () => void refresh(),
      busy: refreshing,
    });
  }, [updatedAt?.getTime(), remaining, refreshing, dashboard?.version]);

  // Editor: a local draft of positions saved as one versioned layout update.
  const [draft, setDraft] = useState<Box[]>([]);
  const [selected, setSelected] = useState<string>();
  const [announcement, setAnnouncement] = useState("");
  const [conflict, setConflict] = useState(false);
  const [problem, setProblem] = useState<Problem>();
  const [adding, setAdding] = useState(false);
  const [sharing, setSharing] = useState<NewWidget>();
  const [revoking, setRevoking] = useState<Widget>();
  useEffect(() => {
    if (editing) {
      setDraft(boxesOf(dashboard));
      setSelected(undefined);
      setProblem(undefined);
    }
  }, [editing]);

  const replaceDashboard = (next: DashboardMessage | undefined) => {
    if (!next) return;
    queryClient.setQueryData(dashboardKey, (previous) =>
      previous ? { ...previous, dashboard: next } : previous,
    );
    // Home renames from its ListDashboards summary, so keep that summary's
    // version and widget count in step with every version-changing edit.
    queryClient.setQueriesData<ListDashboardsResponse>(
      {
        queryKey: createConnectQueryKey({
          schema: DashboardService.method.listDashboards,
          cardinality: "finite",
        }),
      },
      (previous) =>
        previous?.dashboards?.some((entry) => entry.id === next.id)
          ? {
              ...previous,
              dashboards: previous.dashboards.map((entry) =>
                entry.id === next.id
                  ? {
                      ...entry,
                      name: next.name,
                      version: next.version,
                      widgetCount: next.widgets.length,
                      updatedAt: next.updatedAt ?? entry.updatedAt,
                    }
                  : entry,
              ),
            }
          : previous,
    );
  };
  const onMutationError = async (error: unknown) => {
    const next = toProblem(error);
    if (next.kind === ProblemKind.VersionConflict) {
      // Never merge silently: load the latest version and ask for the edit again.
      await dashboardQuery.refetch();
      setEditing(false);
      setConflict(true);
      return;
    }
    setProblem(next);
  };
  const saveLayout = useMutation(DashboardService.method.updateWidgetLayout, {
    onSuccess: (response) => {
      replaceDashboard(response.dashboard);
      setEditing(false);
    },
    onError: onMutationError,
  });
  const addWidget = useMutation(DashboardService.method.addWidget, {
    onSuccess: (response) => {
      replaceDashboard(response.dashboard);
      setAdding(false);
      setSharing(undefined);
      void dataQuery.refetch();
      // Keep unsaved moves and resizes; submitWidget placed the new widget
      // in a free cell of this draft.
      if (editing)
        setDraft((current) =>
          keepDraftPositions(current, boxesOf(response.dashboard)),
        );
    },
    onError: (error) => {
      setAdding(false);
      setSharing(undefined);
      void onMutationError(error);
    },
  });
  const removeWidget = useMutation(DashboardService.method.removeWidget, {
    onSuccess: (response) => {
      replaceDashboard(response.dashboard);
      if (editing)
        setDraft((current) =>
          keepDraftPositions(current, boxesOf(response.dashboard)),
        );
    },
    onError: onMutationError,
  });
  const revoke = useMutation(DashboardService.method.revokeWidgetSharing, {
    onSuccess: (response) => {
      replaceDashboard(response.dashboard);
      setRevoking(undefined);
      void dataQuery.refetch();
    },
    onError: (error) => {
      setRevoking(undefined);
      void onMutationError(error);
    },
  });

  const submitWidget = (widget: NewWidget) => {
    if (!dashboard) return;
    const box = placeNew(editing ? draft : boxesOf(dashboard), "new");
    addWidget.mutate({
      dashboardId: dashboard.id,
      expectedVersion: dashboard.version,
      requestId: uuidv7(),
      type: widget.type,
      layout: {
        x: box.x,
        y: box.y,
        width: box.width,
        height: box.height,
        title: "",
        presentation: widget.presentation,
      },
      query: queryFor(widget),
      source: {
        connectionIds: widget.connectionIds,
        allDevices: widget.allDevices,
      },
    });
  };

  if (blocked) return null;
  // First load without cached data shows the dashboard-shaped skeleton; later
  // refreshes and individual widgets without data show per-widget skeletons.
  if (
    dashboardQuery.isPending ||
    (dataQuery.isPending &&
      cached.size === 0 &&
      (dashboard?.widgets.length ?? 0) > 0)
  )
    return <LoadingState mode={mode} />;
  if (dashboardQuery.isError || !dashboard) {
    const failure = toProblem(dashboardQuery.error);
    if (
      failure.kind === ProblemKind.AccessDenied ||
      failure.kind === ProblemKind.NotFound
    )
      return (
        <AccessDeniedState
          onBack={() => navigate({ kind: RouteKind.Home }, { replace: true })}
        />
      );
    if (failure.kind === ProblemKind.SpaceInactive)
      return <SpaceUnavailableState />;
    if (failure.kind === ProblemKind.UpdateRequired) return null;
    return (
      <ErrorState
        ids={{ correlationId: failure.correlationId }}
        onRetry={() => void dashboardQuery.refetch()}
        onCopy={(text) => void hostCall(HostMethod.CopyText, { text })}
      />
    );
  }
  if (conflict)
    return (
      <RecoveryState
        onContinue={() => {
          setConflict(false);
          setEditing(true);
        }}
      />
    );

  const ctx: ViewContext = {
    timeZone,
    now: new Date(now),
    devices,
    connections,
    nextAttempt,
  };
  const widgets = new Map(
    dashboard.widgets.map((widget) => [widget.id, widget]),
  );
  const boxes = editing ? draft : boxesOf(dashboard);
  const duplicate = duplicateDeviceCount(
    dataQuery.data?.widgets ?? [],
    devices,
  );
  const dataFailure =
    dataQuery.isError && !dataQuery.data
      ? toProblem(dataQuery.error)
      : undefined;
  const viewer =
    session.info.platform === HostPlatform.Ios ||
    session.info.platform === HostPlatform.Android;
  const canRemove = (widget: Widget) =>
    !viewer &&
    can(space, SpacePermission.EDIT_DASHBOARDS) &&
    (widget.callerIsDataOwner || can(space, SpacePermission.DELETE_DASHBOARDS));

  const onKey = (id: string) => (event: KeyEvent) => {
    if (!isEditKey(event.key)) return;
    event.preventDefault?.();
    event.stopPropagation?.();
    const next = applyKey(draft, id, event.key, event.shiftKey === true);
    setDraft(next);
    const box = next.find((entry) => entry.id === id)!;
    const widget = widgets.get(id);
    setAnnouncement(
      describeBox(
        widget ? widgetView(widget, dataById.get(id), ctx).title : "",
        box,
      ),
    );
  };
  const nudge = (key: EditKey, resize: boolean) => {
    if (!selected) return;
    onKey(selected)({ key, shiftKey: resize } as KeyEvent);
  };

  return (
    <view className="lui-stack lui-column lui-gap-8" style={{ width: "100%" }}>
      {duplicate ? <DuplicateNotice devices={duplicate} /> : null}
      {problem?.kind === ProblemKind.QuotaExceeded ? (
        <LimitReached problem={problem} />
      ) : null}
      {editing ? (
        <view className="ad-editor-bar">
          <T muted>{ExtraCopy.editorHint}</T>
          <view className="ad-spacer" />
          <SmallButton onPress={() => setAdding(true)}>
            {ExtraCopy.addWidget}
          </SmallButton>
          {selected ? (
            <view className="lui-stack lui-row lui-gap-4 lui-wrap">
              <SmallButton
                id="editor-move-left"
                onPress={() => nudge(EditKey.Left, false)}
              >
                {ExtraCopy.moveLeft}
              </SmallButton>
              <SmallButton
                id="editor-move-right"
                onPress={() => nudge(EditKey.Right, false)}
              >
                {ExtraCopy.moveRight}
              </SmallButton>
              <SmallButton
                id="editor-move-up"
                onPress={() => nudge(EditKey.Up, false)}
              >
                {ExtraCopy.moveUp}
              </SmallButton>
              <SmallButton
                id="editor-move-down"
                onPress={() => nudge(EditKey.Down, false)}
              >
                {ExtraCopy.moveDown}
              </SmallButton>
              <SmallButton
                id="editor-wider"
                onPress={() => nudge(EditKey.Right, true)}
              >
                {ExtraCopy.wider}
              </SmallButton>
              <SmallButton
                id="editor-narrower"
                onPress={() => nudge(EditKey.Left, true)}
              >
                {ExtraCopy.narrower}
              </SmallButton>
              <SmallButton
                id="editor-taller"
                onPress={() => nudge(EditKey.Down, true)}
              >
                {ExtraCopy.taller}
              </SmallButton>
              <SmallButton
                id="editor-shorter"
                onPress={() => nudge(EditKey.Up, true)}
              >
                {ExtraCopy.shorter}
              </SmallButton>
              {widgets.get(selected) && canRemove(widgets.get(selected)!) ? (
                <SmallButton
                  onPress={() =>
                    removeWidget.mutate({
                      dashboardId: dashboard.id,
                      expectedVersion: dashboard.version,
                      widgetId: selected,
                    })
                  }
                >
                  {Copy.removeWidget}
                </SmallButton>
              ) : null}
              {(() => {
                const widget = widgets.get(selected);
                return widget?.callerIsDataOwner &&
                  widget.sharingState === SharingState.SHARED ? (
                  <SmallButton onPress={() => setRevoking(widget)}>
                    {ExtraCopy.stopSharing}
                  </SmallButton>
                ) : null;
              })()}
            </view>
          ) : null}
          <Button
            onPress={() => {
              const updates = changedBoxes(boxesOf(dashboard), draft).map(
                (box) => ({
                  widgetId: box.id,
                  layout: {
                    title: widgets.get(box.id)?.layout?.title ?? "",
                    presentation: widgets.get(box.id)?.layout?.presentation,
                    x: box.x,
                    y: box.y,
                    width: box.width,
                    height: box.height,
                  },
                }),
              );
              if (updates.length === 0) setEditing(false);
              else
                saveLayout.mutate({
                  dashboardId: dashboard.id,
                  expectedVersion: dashboard.version,
                  updates,
                });
            }}
            loading={saveLayout.isPending}
            loadingLabel={ExtraCopy.saving}
          >
            {Copy.done}
          </Button>
        </view>
      ) : null}
      <A11y role={Role.Status} live label={announcement || undefined}>
        <text className="ad-sr-only">{announcement}</text>
      </A11y>
      {dataFailure && dataFailure.kind !== ProblemKind.UpdateRequired ? (
        <ErrorState
          ids={{ correlationId: dataFailure.correlationId }}
          onRetry={() => void dataQuery.refetch()}
          onCopy={(text) => void hostCall(HostMethod.CopyText, { text })}
        />
      ) : dashboard.widgets.length === 0 ? (
        <view className="ad-screen">
          <view className="ad-center">
            <text className="ad-heading">{ExtraCopy.noWidgets}</text>
            <T muted>{ExtraCopy.noWidgetsDetail}</T>
            {!viewer && can(space, SpacePermission.EDIT_DASHBOARDS) ? (
              <Button onPress={() => setAdding(true)}>
                {ExtraCopy.addWidget}
              </Button>
            ) : null}
          </view>
        </view>
      ) : (
        <Grid
          mode={mode}
          boxes={boxes}
          render={(box) => {
            const widget = widgets.get(box.id);
            if (!widget) return null;
            const view = widgetView(widget, dataById.get(box.id), ctx);
            return (
              <WidgetCard
                view={view}
                compact={mode === LayoutMode.Phone}
                focused={focusWidgetId === box.id}
                onRemove={
                  canRemove(widget)
                    ? () =>
                        removeWidget.mutate({
                          dashboardId: dashboard.id,
                          expectedVersion: dashboard.version,
                          widgetId: widget.id,
                        })
                    : undefined
                }
                editing={
                  editing
                    ? {
                        selected: selected === box.id,
                        description: `${describeBox(view.title, box)} ${ExtraCopy.editorHint}`,
                        onSelect: () => setSelected(box.id),
                        onKey: onKey(box.id),
                      }
                    : undefined
                }
              />
            );
          }}
        />
      )}
      {adding && !sharing ? (
        <AddWidgetDialog
          connections={connections}
          busy={addWidget.isPending}
          onCancel={() => setAdding(false)}
          onAdd={(widget) => {
            // Team dashboards share display data; confirm the exact scope first.
            if (space.kind === SpaceKind.TEAM) setSharing(widget);
            else submitWidget(widget);
          }}
        />
      ) : null}
      {sharing ? (
        <Modal
          title={Copy.shareTitle(space.name)}
          onClose={() => setSharing(undefined)}
          action={{
            label: Copy.shareWidget,
            loading: addWidget.isPending,
            onPress: () => submitWidget(sharing),
          }}
        >
          <text className="ad-text ad-muted">
            {Copy.shareBefore}
            <text className="ad-text ad-strong">{sharing.name}</text>
            {Copy.shareAfter}
          </text>
        </Modal>
      ) : null}
      {revoking ? (
        <Modal
          title={ExtraCopy.stopSharingTitle}
          onClose={() => setRevoking(undefined)}
          action={{
            label: ExtraCopy.stopSharing,
            destructive: true,
            loading: revoke.isPending,
            onPress: () =>
              revoke.mutate({
                dashboardId: dashboard.id,
                widgetId: revoking.id,
              }),
          }}
        >
          <T muted>{ExtraCopy.stopSharingDetail}</T>
        </Modal>
      ) : null}
    </view>
  );
}
