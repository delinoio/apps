// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "@lynx-js/react";
import {
  createConnectQueryKey,
  useMutation,
  useQuery,
} from "@connectrpc/connect-query";
import { useQueryClient } from "@tanstack/react-query";
import { Field, Input, type FocusHandle } from "@packages/lynx-ui";
import {
  AccountService,
  DashboardService,
  OnboardingState,
  SpaceKind,
  SpacePermission,
  type DashboardSummary,
  type Space,
} from "@repo/protos/anydash/v1/anydash_pb";
import { Modal } from "../components/dialog";
import { ConnectDevice } from "../components/ConnectDevice";
import { Pill, SmallButton, T } from "../components/primitives";
import {
  AccessDeniedState,
  ErrorState,
  LimitReached,
  LoadingState,
  NoDashboardsState,
  SpaceUnavailableState,
} from "../components/States";
import { Copy, ExtraCopy } from "../copy";
import { ProblemKind, toProblem, type Problem } from "../errors";
import { formatTime } from "../format";
import { hostCall, HostMethod, HostPlatform } from "../host";
import { LayoutMode } from "../model/grid";
import { RouteKind, type Route } from "../model/routes";
import { useNavigation } from "../navigation";
import { useSession } from "../session";
import { uuidv7 } from "../uuid";
import { can, DashboardScreen } from "./Dashboard";

export const spaceLabel = (space: Space) =>
  space.kind === SpaceKind.PERSONAL ? ExtraCopy.personal : space.name;

enum Dialog {
  None = "none",
  Spaces = "spaces",
  Dashboards = "dashboards",
  Create = "create",
  Rename = "rename",
  Delete = "delete",
}

function NameDialog({
  title,
  initial,
  busy,
  onCancel,
  onSave,
  problem,
}: {
  title: string;
  initial: string;
  busy: boolean;
  onCancel: () => void;
  onSave: (name: string) => void;
  problem?: Problem;
}) {
  const [name, setName] = useState(initial);
  const input = useRef<FocusHandle>(null);
  const valid = name.trim().length > 0 && name.trim().length <= 100;
  return (
    <Modal
      title={title}
      onClose={onCancel}
      initialFocusRef={input}
      action={{
        label: ExtraCopy.save,
        disabled: !valid,
        loading: busy,
        onPress: () => onSave(name.trim()),
      }}
    >
      {problem?.kind === ProblemKind.QuotaExceeded ? (
        <LimitReached problem={problem} />
      ) : null}
      <Field label={ExtraCopy.dashboardName}>
        <Input
          value={name}
          onValueChange={setName}
          accessibilityLabel={ExtraCopy.dashboardName}
          focusRef={input}
          autoFocus
          maxLength={100}
          onConfirm={() => valid && onSave(name.trim())}
        />
      </Field>
    </Modal>
  );
}

export function Home({ route, mode }: { route: Route; mode: LayoutMode }) {
  const session = useSession();
  const { navigate } = useNavigation();
  const queryClient = useQueryClient();
  const routeSpace =
    route.kind === RouteKind.Dashboard
      ? session.spaces.find((space) => space.id === route.spaceId)
      : undefined;
  const [spaceId, setSpaceId] = useState(
    routeSpace?.id ?? session.spaces[0]?.id ?? "",
  );
  useEffect(() => {
    if (routeSpace && routeSpace.id !== spaceId) setSpaceId(routeSpace.id);
  }, [routeSpace?.id]);
  // A dashboard link can name a space the caller no longer belongs to, such as
  // a team they left. Reload the spaces once, in case the caller joined it
  // after sign-in, and then show access denied instead of the selected space.
  const missingSpaceId =
    route.kind === RouteKind.Dashboard && !routeSpace ? route.spaceId : "";
  const [checkedSpaceId, setCheckedSpaceId] = useState("");
  useEffect(() => {
    if (!missingSpaceId || checkedSpaceId === missingSpaceId) return;
    let active = true;
    void session
      .reloadSpaces()
      .catch(() => undefined)
      .finally(() => {
        if (active) setCheckedSpaceId(missingSpaceId);
      });
    return () => {
      active = false;
    };
  }, [missingSpaceId]);
  const space =
    session.spaces.find((entry) => entry.id === spaceId) ?? session.spaces[0];
  const onboarding =
    space?.kind === SpaceKind.PERSONAL &&
    space.onboardingState === OnboardingState.DEVICE_REQUIRED;
  const listKey = createConnectQueryKey({
    schema: DashboardService.method.listDashboards,
    input: { spaceId: space?.id ?? "", page: { size: 50 } },
    transport: session.transport,
    cardinality: "finite",
  });
  const list = useQuery(
    DashboardService.method.listDashboards,
    { spaceId: space?.id ?? "", page: { size: 50 } },
    { enabled: !!space && !onboarding && !session.updateRequired },
  );
  const dashboards = list.data?.dashboards ?? [];
  const current: DashboardSummary | undefined =
    route.kind === RouteKind.Dashboard
      ? dashboards.find((entry) => entry.id === route.dashboardId)
      : undefined;
  const [dialog, setDialog] = useState(Dialog.None);
  const [problem, setProblem] = useState<Problem>();
  const [editing, setEditing] = useState(false);
  const [refresh, setRefresh] = useState<{
    updatedAt?: Date;
    remaining: number;
    refresh: () => void;
    busy: boolean;
  }>();

  // Home opens the first dashboard of the selected space.
  useEffect(() => {
    if (
      route.kind === RouteKind.Home &&
      space &&
      !onboarding &&
      dashboards.length > 0
    )
      navigate(
        {
          kind: RouteKind.Dashboard,
          spaceId: space.id,
          dashboardId: dashboards[0].id,
        },
        { replace: true },
      );
  }, [route.kind, space?.id, dashboards.length]);

  const create = useMutation(DashboardService.method.createDashboard, {
    onSuccess: (response) => {
      setDialog(Dialog.None);
      void queryClient.invalidateQueries({ queryKey: listKey });
      if (response.dashboard && space)
        navigate({
          kind: RouteKind.Dashboard,
          spaceId: space.id,
          dashboardId: response.dashboard.id,
        });
    },
    onError: (error) => setProblem(toProblem(error)),
  });
  const rename = useMutation(DashboardService.method.renameDashboard, {
    onSuccess: () => {
      setDialog(Dialog.None);
      void queryClient.invalidateQueries({ queryKey: listKey });
      void queryClient.invalidateQueries({
        queryKey: createConnectQueryKey({
          schema: DashboardService.method.getDashboard,
          cardinality: undefined,
        }),
      });
    },
    onError: (error) => {
      const next = toProblem(error);
      setProblem(next);
      // Reload the summaries so a retry sends the latest version.
      if (next.kind === ProblemKind.VersionConflict)
        void queryClient.invalidateQueries({ queryKey: listKey });
    },
  });
  const remove = useMutation(DashboardService.method.deleteDashboard, {
    onSuccess: () => {
      setDialog(Dialog.None);
      void queryClient.invalidateQueries({ queryKey: listKey });
      navigate({ kind: RouteKind.Home }, { replace: true });
    },
    onError: (error) => setProblem(toProblem(error)),
  });
  const reloadAccount = useMutation(AccountService.method.getAccount, {
    onSuccess: (response) =>
      response.account && session.setAccount(response.account),
  });

  if (!space) return <LoadingState mode={mode} />;
  const phone = mode === LayoutMode.Phone;
  // iOS and Android apps are viewers; editing happens on web and desktop.
  const viewer =
    session.info.platform === HostPlatform.Ios ||
    session.info.platform === HostPlatform.Android;
  const mayEdit = !viewer && can(space, SpacePermission.EDIT_DASHBOARDS);
  const mayCreate = !viewer && can(space, SpacePermission.CREATE_DASHBOARDS);
  const mayDelete = !viewer && can(space, SpacePermission.DELETE_DASHBOARDS);
  const timeZone = session.account?.timeZone || "UTC";
  const header = (
    <view className="ad-top">
      <text className="ad-logo">{Copy.appName}</text>
      {phone ? <view className="ad-spacer" /> : null}
      <Pill
        label={spaceLabel(space)}
        accessibilityLabel={`${ExtraCopy.chooseSpace}: ${spaceLabel(space)}`}
        onPress={() => {
          void session.reloadSpaces().catch(() => undefined);
          setDialog(Dialog.Spaces);
        }}
      />
      {/* Phones keep the dashboard picker: it is the only way to open another
          dashboard, because Home opens the first one automatically. */}
      {!onboarding ? (
        <Pill
          label={current?.name ?? ExtraCopy.dashboards}
          accessibilityLabel={`${ExtraCopy.chooseDashboard}: ${current?.name ?? ExtraCopy.dashboards}`}
          onPress={() => setDialog(Dialog.Dashboards)}
        />
      ) : null}
      {!phone ? <view className="ad-spacer" /> : null}
      {!phone && refresh?.updatedAt ? (
        <text className="ad-small">
          {Copy.headerUpdated(
            timeZone,
            formatTime(refresh.updatedAt, timeZone),
          )}
        </text>
      ) : null}
      {current && refresh ? (
        <view className="lui-stack lui-column lui-gap-4">
          <SmallButton
            onPress={refresh.refresh}
            disabled={refresh.remaining > 0 || refresh.busy}
            accessibilityLabel={
              refresh.remaining > 0
                ? `${Copy.refresh}. ${Copy.refreshAvailableIn(refresh.remaining)}`
                : Copy.refresh
            }
          >
            {Copy.refresh}
          </SmallButton>
        </view>
      ) : null}
      {current && mayEdit && !phone ? (
        <SmallButton onPress={() => setEditing(!editing)}>
          {editing ? Copy.cancel : Copy.edit}
        </SmallButton>
      ) : null}
      <SmallButton onPress={() => navigate({ kind: RouteKind.Settings })}>
        {ExtraCopy.settings}
      </SmallButton>
    </view>
  );
  let content;
  if (session.updateRequired) content = null;
  else if (missingSpaceId)
    content =
      checkedSpaceId === missingSpaceId ? (
        <AccessDeniedState
          onBack={() => navigate({ kind: RouteKind.Home }, { replace: true })}
        />
      ) : (
        <LoadingState mode={mode} />
      );
  else if (onboarding)
    content = (
      <ConnectDevice
        onConnected={() => {
          void session.reloadSpaces();
          reloadAccount.mutate({});
        }}
      />
    );
  else if (list.isPending) content = <LoadingState mode={mode} />;
  else if (list.isError) {
    const failure = toProblem(list.error);
    content =
      failure.kind === ProblemKind.SpaceInactive ? (
        <SpaceUnavailableState />
      ) : failure.kind === ProblemKind.OnboardingRequired ? (
        <ConnectDevice onConnected={() => void session.reloadSpaces()} />
      ) : (
        <ErrorState
          ids={{ correlationId: failure.correlationId }}
          onRetry={() => void list.refetch()}
          onCopy={(text) => void hostCall(HostMethod.CopyText, { text })}
        />
      );
  } else if (dashboards.length === 0)
    content = (
      <NoDashboardsState
        onCreate={mayCreate ? () => setDialog(Dialog.Create) : undefined}
      />
    );
  else if (route.kind === RouteKind.Dashboard && route.spaceId === space.id)
    content = (
      <DashboardScreen
        key={route.dashboardId}
        space={space}
        dashboardId={route.dashboardId}
        focusWidgetId={route.widgetId}
        mode={mode}
        onRefreshState={setRefresh}
        editing={editing}
        setEditing={setEditing}
      />
    );
  else content = <LoadingState mode={mode} />;

  return (
    <view className="lui-stack lui-column lui-gap-16" style={{ width: "100%" }}>
      {header}
      {refresh && refresh.remaining > 0 ? (
        <view className="ad-tooltip" style={{ alignSelf: "flex-end" }}>
          <text className="ad-small">
            {Copy.refreshAvailableIn(refresh.remaining)}
          </text>
        </view>
      ) : null}
      {content}
      {dialog === Dialog.Spaces ? (
        <Modal
          title={ExtraCopy.chooseSpace}
          onClose={() => setDialog(Dialog.None)}
        >
          {session.spaces.map((entry) => (
            <SmallButton
              key={entry.id}
              accessibilityLabel={`${spaceLabel(entry)}${entry.id === space.id ? ", selected" : ""}`}
              onPress={() => {
                setSpaceId(entry.id);
                setEditing(false);
                setDialog(Dialog.None);
                navigate({ kind: RouteKind.Home });
              }}
            >
              {entry.id === space.id
                ? `✓ ${spaceLabel(entry)}`
                : spaceLabel(entry)}
            </SmallButton>
          ))}
        </Modal>
      ) : null}
      {dialog === Dialog.Dashboards ? (
        <Modal
          title={ExtraCopy.chooseDashboard}
          onClose={() => setDialog(Dialog.None)}
        >
          {dashboards.map((entry) => (
            <SmallButton
              key={entry.id}
              onPress={() => {
                setEditing(false);
                setDialog(Dialog.None);
                navigate({
                  kind: RouteKind.Dashboard,
                  spaceId: space.id,
                  dashboardId: entry.id,
                });
              }}
            >
              {entry.id === current?.id ? `✓ ${entry.name}` : entry.name}
            </SmallButton>
          ))}
          <view className="lui-stack lui-row lui-gap-8 lui-wrap">
            {mayCreate ? (
              <SmallButton onPress={() => setDialog(Dialog.Create)}>
                {Copy.createDashboard}
              </SmallButton>
            ) : null}
            {current && mayEdit ? (
              <SmallButton onPress={() => setDialog(Dialog.Rename)}>
                {ExtraCopy.renameDashboard}
              </SmallButton>
            ) : null}
            {current && mayDelete ? (
              <SmallButton onPress={() => setDialog(Dialog.Delete)}>
                {Copy.deleteDashboard}
              </SmallButton>
            ) : null}
          </view>
        </Modal>
      ) : null}
      {dialog === Dialog.Create ? (
        <NameDialog
          title={Copy.createDashboard}
          initial=""
          busy={create.isPending}
          problem={problem}
          onCancel={() => {
            setProblem(undefined);
            setDialog(Dialog.None);
          }}
          onSave={(name) =>
            create.mutate({ requestId: uuidv7(), spaceId: space.id, name })
          }
        />
      ) : null}
      {dialog === Dialog.Rename && current ? (
        <NameDialog
          title={ExtraCopy.renameDashboard}
          initial={current.name}
          busy={rename.isPending}
          problem={problem}
          onCancel={() => setDialog(Dialog.None)}
          onSave={(name) =>
            rename.mutate({
              dashboardId: current.id,
              expectedVersion: current.version,
              name,
            })
          }
        />
      ) : null}
      {dialog === Dialog.Delete && current ? (
        <Modal
          title={Copy.deleteDashboardTitle(current.name)}
          onClose={() => setDialog(Dialog.None)}
          action={{
            label: Copy.deleteDashboard,
            destructive: true,
            loading: remove.isPending,
            onPress: () => remove.mutate({ dashboardId: current.id }),
          }}
        >
          <T muted>{Copy.deleteDashboardDetail(current.widgetCount)}</T>
        </Modal>
      ) : null}
      {problem &&
      dialog === Dialog.None &&
      problem.kind === ProblemKind.VersionConflict ? (
        <T muted>{Copy.dashboardChangedDetail}</T>
      ) : null}
    </view>
  );
}
