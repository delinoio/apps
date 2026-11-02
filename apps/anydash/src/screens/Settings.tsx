// SPDX-License-Identifier: Apache-2.0
import { pick } from "../format";
import { useEffect, useRef, useState, type ReactNode } from "@lynx-js/react";
import { createClient } from "@connectrpc/connect";
import {
  createConnectQueryKey,
  useMutation,
  useQuery,
} from "@connectrpc/connect-query";
import { useQueryClient } from "@tanstack/react-query";
import {
  Button,
  Field,
  Input,
  Switch,
  Variant,
  type FocusHandle,
} from "@packages/lynx-ui";
import {
  AccountService,
  ActivityKind,
  ClientKind,
  CollectorRunState,
  ConnectionService,
  ConnectionState,
  DeviceService,
  KeySource,
  LocalCollectorService,
  LocalTool,
  Provider,
  type CollectorStatus,
  type Connection,
  type SourcePath,
} from "@repo/protos/anydash/v1/anydash_pb";
import { Modal } from "../components/dialog";
import { LinkText, SmallButton, StateBadge, T } from "../components/primitives";
import { LimitReached } from "../components/States";
import { Copy, ExtraCopy } from "../copy";
import { ProblemKind, toProblem, type Problem } from "../errors";
import {
  formatDateTime,
  providerName,
  toolName,
  usesLocalLogin,
} from "../format";
import {
  APP_VERSION,
  hostCall,
  HostMethod,
  UpdateAction,
  UpdatePhase,
  type UpdateStatus,
} from "../host";
import { BadgeTone } from "../model/widget-view";
import { useNavigation } from "../navigation";
import { useSession } from "../session";
import { RouteKind } from "../model/routes";
import { platformLabel } from "./DeviceApproval";

export const ACCOUNT_DELETION_URL = "https://delino.io/account";
export const SUPPORT_ISSUES_URL = "https://github.com/delinoio/apps/issues";
export const SECURITY_REPORT_URL =
  "https://github.com/delinoio/apps/security/advisories/new";

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <view className="ad-section">
      <view
        accessibility-element
        accessibility-label={title}
        accessibility-role-description="heading"
      >
        <text className="ad-section-title">{title}</text>
      </view>
      {children}
    </view>
  );
}

function ProblemText({ problem }: { problem?: Problem }) {
  if (!problem) return null;
  if (problem.kind === ProblemKind.QuotaExceeded)
    return <LimitReached problem={problem} />;
  if (problem.kind === ProblemKind.RateLimited)
    return (
      <T muted>{Copy.refreshAvailableIn(problem.retryAfterSeconds ?? 60)}</T>
    );
  return (
    <T className="ad-text ad-danger-text">
      {`${problem.message || ExtraCopy.actionFailed}${
        problem.correlationId
          ? ` (${Copy.correlationId} ${problem.correlationId})`
          : ""
      }`}
    </T>
  );
}

const open = (url: string) => void hostCall(HostMethod.OpenURL, { url });

function TimeZoneSection() {
  const session = useSession();
  const [value, setValue] = useState(session.account?.timeZone ?? "");
  const [problem, setProblem] = useState<Problem>();
  const update = useMutation(AccountService.method.updateAccountSettings, {
    onSuccess: (response) => {
      setProblem(undefined);
      if (response.account) session.setAccount(response.account);
    },
    onError: (error) => setProblem(toProblem(error)),
  });
  return (
    <Section title={ExtraCopy.timeZone}>
      <T muted>{ExtraCopy.timeZoneDetail}</T>
      <Field label={ExtraCopy.timeZone} hint={ExtraCopy.timeZoneHint}>
        <Input
          value={value}
          onValueChange={setValue}
          accessibilityLabel={ExtraCopy.timeZone}
          onConfirm={() => update.mutate({ timeZone: value.trim() })}
        />
      </Field>
      <view className="lui-stack lui-row lui-gap-8">
        <Button
          onPress={() => update.mutate({ timeZone: value.trim() })}
          loading={update.isPending}
          disabled={!value.trim()}
        >
          {ExtraCopy.save}
        </Button>
      </view>
      <ProblemText problem={problem} />
    </Section>
  );
}

function DevicesSection() {
  const session = useSession();
  const queryClient = useQueryClient();
  const devices = useQuery(DeviceService.method.listDevices, {
    page: { size: 200 },
  });
  const key = createConnectQueryKey({
    schema: DeviceService.method.listDevices,
    cardinality: undefined,
  });
  const [problem, setProblem] = useState<Problem>();
  const [deleting, setDeleting] = useState<string>();
  const onError = (error: unknown) => setProblem(toProblem(error));
  const refresh = () => void queryClient.invalidateQueries({ queryKey: key });
  const preferred = useMutation(DeviceService.method.setPreferredDevice, {
    onSuccess: refresh,
    onError,
  });
  const sync = useMutation(DeviceService.method.requestDeviceSync, { onError });
  const remove = useMutation(DeviceService.method.deleteDevice, {
    onSuccess: () => {
      setDeleting(undefined);
      refresh();
    },
    onError,
  });
  const list = devices.data?.devices ?? [];
  const tz = session.account?.timeZone || "UTC";
  return (
    <Section title={ExtraCopy.devices}>
      {list.length === 0 && !devices.isPending ? (
        <T muted>{ExtraCopy.noDevices}</T>
      ) : null}
      {list.map((device) => (
        <view key={device.id} className="ad-list-item">
          <view className="ad-list-main">
            <T strong>{device.name}</T>
            <T className="ad-small">
              {`${platformLabel(device.platform, device.architecture)} · ${Copy.collector} ${device.collectorVersion} · ${
                device.online
                  ? ExtraCopy.online
                  : `${ExtraCopy.lastSeen} ${formatDateTime(device.lastHeartbeatAt, tz)}`
              }`}
            </T>
            <text className="ad-mono ad-muted">{`${Copy.deviceId} ${device.id}`}</text>
          </view>
          {device.preferred ? (
            <StateBadge
              badge={{ tone: BadgeTone.Info, label: ExtraCopy.preferred }}
            />
          ) : (
            <SmallButton
              onPress={() => preferred.mutate({ deviceId: device.id })}
            >
              {ExtraCopy.setPreferred}
            </SmallButton>
          )}
          <SmallButton onPress={() => sync.mutate({ deviceId: device.id })}>
            {ExtraCopy.syncNow}
          </SmallButton>
          <SmallButton onPress={() => setDeleting(device.id)}>
            {ExtraCopy.delete}
          </SmallButton>
        </view>
      ))}
      <ProblemText problem={problem} />
      {deleting ? (
        <Modal
          title={ExtraCopy.deleteDeviceTitle}
          onClose={() => setDeleting(undefined)}
          action={{
            label: ExtraCopy.deleteDevice,
            destructive: true,
            loading: remove.isPending,
            onPress: () => remove.mutate({ deviceId: deleting }),
          }}
        >
          <T muted>{ExtraCopy.deleteDeviceDetail}</T>
        </Modal>
      ) : null}
    </Section>
  );
}

const connectionBadge = (connection: Connection) => {
  switch (connection.state) {
    case ConnectionState.OK:
      return { tone: BadgeTone.Success, label: ExtraCopy.connectionOk };
    case ConnectionState.AUTH_EXPIRED:
      return { tone: BadgeTone.Warning, label: Copy.authExpired };
    case ConnectionState.ERROR:
      return { tone: BadgeTone.Danger, label: Copy.couldNotRefresh };
    case ConnectionState.WAITING_FOR_FIRST_RESULT:
      return { tone: BadgeTone.Neutral, label: ExtraCopy.waitingFirstResult };
    case ConnectionState.NO_RUNNING_COLLECTOR:
      return { tone: BadgeTone.Neutral, label: ExtraCopy.noRunningCollector };
    case ConnectionState.NO_REGISTERED_DEVICE:
      return { tone: BadgeTone.Neutral, label: ExtraCopy.noRegisteredDevice };
    default:
      return { tone: BadgeTone.Neutral, label: ExtraCopy.unknownState };
  }
};

const tokenProviders = [
  Provider.VERCEL,
  Provider.SUPABASE,
  Provider.CLOUDFLARE,
];
const allProviders = [
  Provider.CODEX,
  Provider.CLAUDE_CODE,
  Provider.GROK,
  ...tokenProviders,
];

/**
 * Credentials are added only through the local collector, which validates and
 * stores them on this device; they never reach the server.
 */
function ConnectDialog({
  provider,
  connectionId,
  onClose,
  onDone,
}: {
  provider: Provider;
  connectionId?: string;
  onClose: () => void;
  onDone: () => void;
}) {
  const session = useSession();
  const [secret, setSecret] = useState("");
  const [scope, setScope] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<Problem>();
  const token = useRef<FocusHandle>(null);
  const scopeInput = useRef<FocusHandle>(null);
  const needsToken = tokenProviders.includes(provider);
  const vercel = provider === Provider.VERCEL;
  const submit = async () => {
    if (!session.collector || (needsToken && !secret.trim())) return;
    setBusy(true);
    try {
      await createClient(
        LocalCollectorService,
        session.collector,
      ).addConnectionCredential({
        provider,
        connectionId: connectionId ?? "",
        displayName: "",
        secret: needsToken ? secret.trim() : "",
        scopeId: vercel ? scope.trim() : "",
      });
      setSecret("");
      onDone();
    } catch (error) {
      setProblem(toProblem(error));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal
      title={Copy.connectProvider(providerName(provider))}
      onClose={onClose}
      initialFocusRef={needsToken ? token : undefined}
      action={{
        label: Copy.connect,
        loading: busy,
        disabled: needsToken && !secret.trim(),
        onPress: () => void submit(),
      }}
    >
      {vercel ? (
        <view
          className="ad-notice ad-notice-warning"
          accessibility-element
          accessibility-label={Copy.vercelWarning}
          accessibility-role-description="alert"
        >
          <text className="ad-notice-icon">!</text>
          <text className="ad-notice-body">{Copy.vercelWarning}</text>
        </view>
      ) : null}
      {needsToken ? (
        <Field label={vercel ? Copy.teamScopedToken : ExtraCopy.apiToken}>
          <Input
            value={secret}
            onValueChange={setSecret}
            accessibilityLabel={
              vercel ? Copy.teamScopedToken : ExtraCopy.apiToken
            }
            focusRef={token}
            autoFocus
          />
        </Field>
      ) : (
        <T muted>{ExtraCopy.localLoginDetail(providerName(provider))}</T>
      )}
      {vercel ? (
        <Field label={ExtraCopy.vercelTeam} hint={ExtraCopy.vercelTeamHint}>
          <Input
            value={scope}
            onValueChange={setScope}
            accessibilityLabel={ExtraCopy.vercelTeam}
            focusRef={scopeInput}
          />
        </Field>
      ) : null}
      <ProblemText problem={problem} />
    </Modal>
  );
}

function ConnectionsSection({ status }: { status?: CollectorStatus }) {
  const session = useSession();
  const queryClient = useQueryClient();
  const connections = useQuery(ConnectionService.method.listConnections, {
    page: { size: 200 },
  });
  const key = createConnectQueryKey({
    schema: ConnectionService.method.listConnections,
    cardinality: undefined,
  });
  const [problem, setProblem] = useState<Problem>();
  const [connecting, setConnecting] = useState<{
    provider: Provider;
    connectionId?: string;
  }>();
  const [choosing, setChoosing] = useState(false);
  const [deleting, setDeleting] = useState<Connection>();
  const refresh = () => void queryClient.invalidateQueries({ queryKey: key });
  const onError = (error: unknown) => setProblem(toProblem(error));
  const manual = useMutation(
    ConnectionService.method.requestConnectionRefresh,
    { onError },
  );
  const remove = useMutation(ConnectionService.method.deleteConnection, {
    onSuccess: () => {
      setDeleting(undefined);
      refresh();
    },
    onError,
  });
  const local = new Map(
    (status?.connections ?? []).map((entry) => [entry.connectionId, entry]),
  );
  const removeLocal = async (connectionId: string) => {
    if (!session.collector) return;
    try {
      await createClient(
        LocalCollectorService,
        session.collector,
      ).removeConnectionCredential({
        connectionId,
      });
      refresh();
    } catch (error) {
      onError(error);
    }
  };
  return (
    <Section title={ExtraCopy.connections}>
      {(connections.data?.connections ?? []).map((connection) => (
        <view key={connection.id} className="ad-list-item">
          <view className="ad-list-main">
            <T
              strong
            >{`${providerName(connection.provider)} · ${connection.displayName}`}</T>
            {connection.accountLabel ? (
              <T className="ad-small">{connection.accountLabel}</T>
            ) : null}
            {connection.state === ConnectionState.AUTH_EXPIRED ? (
              <T className="ad-small">
                {usesLocalLogin(connection.provider)
                  ? ExtraCopy.cliResumes
                  : Copy.authExpiredToken(providerName(connection.provider))}
              </T>
            ) : null}
            <text className="ad-mono ad-muted">{`${Copy.connectionId} ${connection.id}`}</text>
          </view>
          <StateBadge badge={connectionBadge(connection)} />
          <SmallButton
            onPress={() => manual.mutate({ connectionId: connection.id })}
          >
            {Copy.refresh}
          </SmallButton>
          {session.collector ? (
            local.get(connection.id)?.credentialPresent ? (
              <SmallButton onPress={() => void removeLocal(connection.id)}>
                {ExtraCopy.removeFromComputer}
              </SmallButton>
            ) : (
              <SmallButton
                onPress={() =>
                  setConnecting({
                    provider: connection.provider,
                    connectionId: connection.id,
                  })
                }
              >
                {ExtraCopy.useOnComputer}
              </SmallButton>
            )
          ) : null}
          <SmallButton onPress={() => setDeleting(connection)}>
            {ExtraCopy.delete}
          </SmallButton>
        </view>
      ))}
      {session.collector ? (
        <view className="lui-stack lui-row lui-gap-8">
          <Button variant={Variant.Outline} onPress={() => setChoosing(true)}>
            {ExtraCopy.addConnection}
          </Button>
        </view>
      ) : (
        <T muted>{ExtraCopy.addConnectionElsewhere}</T>
      )}
      <ProblemText problem={problem} />
      {choosing ? (
        <Modal
          title={ExtraCopy.addConnection}
          onClose={() => setChoosing(false)}
        >
          {allProviders.map((provider) => (
            <SmallButton
              key={provider}
              onPress={() => {
                setChoosing(false);
                setConnecting({ provider });
              }}
            >
              {providerName(provider)}
            </SmallButton>
          ))}
        </Modal>
      ) : null}
      {connecting ? (
        <ConnectDialog
          provider={connecting.provider}
          connectionId={connecting.connectionId}
          onClose={() => setConnecting(undefined)}
          onDone={() => {
            setConnecting(undefined);
            refresh();
          }}
        />
      ) : null}
      {deleting ? (
        <Modal
          title={ExtraCopy.deleteConnectionTitle(
            providerName(deleting.provider),
          )}
          onClose={() => setDeleting(undefined)}
          action={{
            label: ExtraCopy.deleteConnection,
            destructive: true,
            loading: remove.isPending,
            onPress: () => remove.mutate({ connectionId: deleting.id }),
          }}
        >
          <T muted>{ExtraCopy.deleteConnectionDetail}</T>
        </Modal>
      ) : null}
    </Section>
  );
}

const runStateLabel = (state: CollectorRunState | undefined) =>
  pick(
    {
      [CollectorRunState.RUNNING]: ExtraCopy.collectorRunning,
      [CollectorRunState.NOT_LOGGED_IN]: ExtraCopy.collectorNotLoggedIn,
      [CollectorRunState.OFFLINE]: ExtraCopy.collectorOffline,
      [CollectorRunState.UPDATE_REQUIRED]: Copy.updateRequired,
      [CollectorRunState.CREDENTIALS_INACTIVE]: ExtraCopy.collectorInactive,
    },
    state ?? CollectorRunState.UNSPECIFIED,
    ExtraCopy.unknownState,
  );

const keySourceLabel = (source: KeySource | undefined) =>
  pick(
    {
      [KeySource.OS_SECURE_STORE]: ExtraCopy.keyOs,
      [KeySource.KEY_FILE]: ExtraCopy.keyFile,
      [KeySource.ENVIRONMENT]: ExtraCopy.keyEnvironment,
    },
    source ?? KeySource.UNSPECIFIED,
    ExtraCopy.unknownState,
  );

function ComputerSection({
  status,
  reload,
}: {
  status?: CollectorStatus;
  reload: () => void;
}) {
  const session = useSession();
  const collector = createClient(LocalCollectorService, session.collector!);
  const [paths, setPaths] = useState<SourcePath[]>([]);
  const [newPath, setNewPath] = useState("");
  const [tool, setTool] = useState(LocalTool.CODEX);
  const [autostart, setAutostart] = useState<boolean>();
  const [problem, setProblem] = useState<Problem>();
  const [rotating, setRotating] = useState(false);
  const [rotated, setRotated] = useState("");
  const tz = session.account?.timeZone || "UTC";
  const loadPaths = () =>
    collector
      .listSourcePaths({})
      .then((response) => setPaths(response.paths))
      .catch((error) => setProblem(toProblem(error)));
  useEffect(() => {
    void loadPaths();
    void hostCall<{ enabled: boolean }>(HostMethod.Autostart, {})
      .then((value) => setAutostart(value.enabled))
      .catch(() => setAutostart(undefined));
  }, []);
  const run = async (action: () => Promise<unknown>) => {
    setProblem(undefined);
    try {
      await action();
      reload();
      await loadPaths();
    } catch (error) {
      setProblem(toProblem(error));
    }
  };
  return (
    <Section title={ExtraCopy.thisComputer}>
      <view className="ad-ids">
        <text className="ad-mono">{`${ExtraCopy.collectorState} ${runStateLabel(status?.runState)}`}</text>
        {status?.deviceId ? (
          <text className="ad-mono">{`${Copy.deviceId} ${status.deviceId}`}</text>
        ) : null}
        <text className="ad-mono">{`${Copy.collector} ${status?.collectorVersion ?? ExtraCopy.unknownState}`}</text>
        {status?.lastLocalSyncAt ? (
          <text className="ad-mono">{`${ExtraCopy.lastSync} ${formatDateTime(status.lastLocalSyncAt, tz)}`}</text>
        ) : null}
        {status?.lastUploadAt ? (
          <text className="ad-mono">{`${ExtraCopy.lastUpload} ${formatDateTime(status.lastUploadAt, tz)}`}</text>
        ) : null}
        <text className="ad-mono">{`${ExtraCopy.pendingBuckets} ${status?.pendingBucketCount ?? 0n}`}</text>
        <text className="ad-mono">{`${ExtraCopy.keySource} ${keySourceLabel(status?.keySource)}`}</text>
        {status?.lastErrorCorrelationId ? (
          <text className="ad-mono">{`${Copy.correlationId} ${status.lastErrorCorrelationId}`}</text>
        ) : null}
      </view>
      <view className="lui-stack lui-row lui-gap-8 lui-wrap">
        <SmallButton
          onPress={() => void run(() => collector.requestLocalSync({}))}
        >
          {ExtraCopy.syncNow}
        </SmallButton>
        <SmallButton
          onPress={() =>
            void hostCall(HostMethod.CopyText, {
              text: [
                status?.deviceId && `${Copy.deviceId} ${status.deviceId}`,
                status?.collectorVersion &&
                  `${Copy.collector} ${status.collectorVersion}`,
                status?.lastErrorCorrelationId &&
                  `${Copy.correlationId} ${status.lastErrorCorrelationId}`,
              ]
                .filter(Boolean)
                .join("\n"),
            })
          }
        >
          {Copy.copyDetails}
        </SmallButton>
      </view>
      {autostart !== undefined ? (
        <Switch
          checked={autostart}
          label={ExtraCopy.startAtLogin}
          checkedLabel={ExtraCopy.on}
          uncheckedLabel={ExtraCopy.off}
          onCheckedChange={(enabled) =>
            void hostCall<{ enabled: boolean }>(HostMethod.Autostart, {
              enabled,
            })
              .then((value) => setAutostart(value.enabled))
              .catch((error) => setProblem(toProblem(error)))
          }
        />
      ) : null}
      <T strong>{ExtraCopy.sessionPaths}</T>
      {paths.map((path) => (
        <view key={`${path.tool}:${path.path}`} className="ad-list-item">
          <view className="ad-list-main">
            <text className="ad-mono">{path.path}</text>
            <T className="ad-small">
              {`${toolName(path.tool)} · ${path.exists ? ExtraCopy.pathFound : ExtraCopy.pathMissing}`}
            </T>
          </view>
          <SmallButton
            onPress={() =>
              void run(() =>
                path.enabled
                  ? collector.disableSourcePath({
                      tool: path.tool,
                      path: path.path,
                    })
                  : collector.enableSourcePath({
                      tool: path.tool,
                      path: path.path,
                    }),
              )
            }
          >
            {path.enabled ? ExtraCopy.disable : ExtraCopy.enable}
          </SmallButton>
        </view>
      ))}
      <view className="lui-stack lui-row lui-gap-8 lui-wrap">
        <SmallButton
          onPress={() =>
            setTool(
              tool === LocalTool.CODEX
                ? LocalTool.CLAUDE_CODE
                : LocalTool.CODEX,
            )
          }
        >
          {toolName(tool)}
        </SmallButton>
      </view>
      <Field label={ExtraCopy.addPath}>
        <Input
          value={newPath}
          onValueChange={setNewPath}
          accessibilityLabel={ExtraCopy.addPath}
        />
      </Field>
      <view className="lui-stack lui-row lui-gap-8 lui-wrap">
        <SmallButton
          disabled={!newPath.trim()}
          onPress={() =>
            void run(async () => {
              await collector.addSourcePath({ tool, path: newPath.trim() });
              setNewPath("");
            })
          }
        >
          {ExtraCopy.addPathAction}
        </SmallButton>
        <SmallButton onPress={() => setRotating(true)}>
          {ExtraCopy.rotateKey}
        </SmallButton>
      </view>
      {rotated ? <T muted>{rotated}</T> : null}
      <ProblemText problem={problem} />
      {rotating ? (
        <Modal
          title={ExtraCopy.rotateKeyTitle}
          onClose={() => setRotating(false)}
          action={{
            label: ExtraCopy.rotateKey,
            onPress: () =>
              void run(async () => {
                const response = await collector.rotateKey({});
                setRotated(ExtraCopy.rotatedKey(response.secretCount));
                setRotating(false);
              }),
          }}
        >
          <T muted>{ExtraCopy.rotateKeyDetail}</T>
        </Modal>
      ) : null}
    </Section>
  );
}

function UpdatesSection() {
  const [status, setStatus] = useState<UpdateStatus>();
  const [confirming, setConfirming] = useState(false);
  const act = (action: UpdateAction) =>
    hostCall<UpdateStatus>(HostMethod.Update, { action })
      .then(setStatus)
      .catch(() => undefined);
  useEffect(() => {
    void act(UpdateAction.Status);
  }, []);
  useEffect(() => {
    if (
      !status ||
      ![
        UpdatePhase.Checking,
        UpdatePhase.Downloading,
        UpdatePhase.Verifying,
        UpdatePhase.Installing,
      ].includes(status.phase)
    )
      return;
    const timer = setInterval(() => void act(UpdateAction.Status), 1000);
    return () => clearInterval(timer);
  }, [status?.phase]);
  if (!status) return null;
  return (
    <Section title={ExtraCopy.appUpdates}>
      <T muted>{status.message}</T>
      {status.phase === UpdatePhase.Ready ? (
        <Button onPress={() => setConfirming(true)}>
          {ExtraCopy.installUpdate}
        </Button>
      ) : status.phase !== UpdatePhase.Unavailable ? (
        <Button
          variant={Variant.Outline}
          onPress={() => void act(UpdateAction.Check)}
          disabled={[
            UpdatePhase.Checking,
            UpdatePhase.Downloading,
            UpdatePhase.Verifying,
            UpdatePhase.Installing,
          ].includes(status.phase)}
        >
          {ExtraCopy.checkUpdates}
        </Button>
      ) : null}
      {confirming ? (
        <Modal
          title={ExtraCopy.installUpdateTitle(status.availableVersion ?? "")}
          onClose={() => setConfirming(false)}
          action={{
            label: ExtraCopy.installUpdate,
            onPress: () => {
              setConfirming(false);
              void act(UpdateAction.Install);
            },
          }}
        >
          <T muted>{ExtraCopy.installUpdateDetail}</T>
        </Modal>
      ) : null}
    </Section>
  );
}

const activityLabel = (kind: ActivityKind) =>
  ActivityKind[kind]
    .toLowerCase()
    .split("_")
    .map((word, index) =>
      index === 0 ? word[0].toUpperCase() + word.slice(1) : word,
    )
    .join(" ");

function ActivitySection() {
  const session = useSession();
  const activity = useQuery(AccountService.method.listActivity, {
    spaceId: "",
    page: { size: 50 },
  });
  const tz = session.account?.timeZone || "UTC";
  return (
    <Section title={ExtraCopy.activity}>
      <T muted>{ExtraCopy.activityDetail}</T>
      {(activity.data?.events ?? []).map((event) => (
        <view key={event.id} className="ad-list-item">
          <view className="ad-list-main">
            <T>{activityLabel(event.kind)}</T>
            <T className="ad-small">
              {`${formatDateTime(event.occurredAt, tz)} · ${
                ClientKind[event.clientKind]
                  ?.toLowerCase()
                  .replace(/_/g, " ") ?? ""
              }`}
            </T>
          </view>
        </view>
      ))}
    </Section>
  );
}

function DeleteDataSection() {
  const session = useSession();
  const [confirming, setConfirming] = useState(false);
  const [problem, setProblem] = useState<Problem>();
  const remove = useMutation(AccountService.method.deleteAccountData, {
    onSuccess: () => {
      setConfirming(false);
      void session.signOut();
    },
    onError: (error) => setProblem(toProblem(error)),
  });
  return (
    <Section title={Copy.deleteData}>
      <T muted>{ExtraCopy.deleteDataSummary}</T>
      <view className="lui-stack lui-row lui-gap-8">
        <Button variant={Variant.Danger} onPress={() => setConfirming(true)}>
          {Copy.deleteData}
        </Button>
      </view>
      <ProblemText problem={problem} />
      {confirming ? (
        <Modal
          title={Copy.deleteDataTitle}
          onClose={() => setConfirming(false)}
          action={{
            label: Copy.deleteData,
            destructive: true,
            loading: remove.isPending,
            onPress: () => remove.mutate({ confirm: true }),
          }}
        >
          <text className="ad-text ad-muted">
            {Copy.deleteDataBefore}
            <text
              className="ad-text ad-link"
              bindtap={() => open(ACCOUNT_DELETION_URL)}
              accessibility-element
              accessibility-label={Copy.deleteDataLink}
              accessibility-role-description="link"
            >
              {Copy.deleteDataLink}
            </text>
            {Copy.deleteDataAfter}
          </text>
        </Modal>
      ) : null}
    </Section>
  );
}

export function Settings() {
  const session = useSession();
  const { navigate } = useNavigation();
  const [status, setStatus] = useState<CollectorStatus>();
  const reload = () => {
    if (!session.collector) return;
    void createClient(LocalCollectorService, session.collector)
      .getCollectorStatus({})
      .then((response) => setStatus(response.status))
      .catch(() => setStatus(undefined));
  };
  useEffect(reload, []);
  return (
    <view
      className="lui-stack lui-column lui-gap-16 ad-settings"
      style={{ width: "100%", alignSelf: "center" }}
    >
      <view className="ad-top">
        <text className="ad-logo">{Copy.appName}</text>
        <view className="ad-spacer" />
        <SmallButton onPress={() => navigate({ kind: RouteKind.Home })}>
          {ExtraCopy.back}
        </SmallButton>
      </view>
      <view
        accessibility-element
        accessibility-label={ExtraCopy.settings}
        accessibility-role-description="heading"
      >
        <text className="ad-heading">{ExtraCopy.settings}</text>
      </view>
      {session.updateRequired ? null : <TimeZoneSection />}
      {session.collector ? (
        <ComputerSection status={status} reload={reload} />
      ) : null}
      {session.collector ? <UpdatesSection /> : null}
      {session.updateRequired ? null : <DevicesSection />}
      {session.updateRequired ? null : <ConnectionsSection status={status} />}
      {session.updateRequired ? null : <ActivitySection />}
      {session.updateRequired ? null : <DeleteDataSection />}
      <Section title={ExtraCopy.about}>
        <T>{`${Copy.appName} ${APP_VERSION} · ${session.info.platformLabel}`}</T>
        {status?.collectorVersion ? (
          <T>{`${Copy.collector} ${status.collectorVersion}`}</T>
        ) : null}
        <LinkText onPress={() => open(SUPPORT_ISSUES_URL)}>
          {ExtraCopy.reportIssue}
        </LinkText>
        <T className="ad-small">{ExtraCopy.noPersonalData}</T>
        <LinkText onPress={() => open(SECURITY_REPORT_URL)}>
          {ExtraCopy.reportSecurity}
        </LinkText>
      </Section>
      <view className="lui-stack lui-row lui-gap-8">
        <Button
          variant={Variant.Outline}
          onPress={() => void session.signOut()}
        >
          {ExtraCopy.signOut}
        </Button>
      </view>
    </view>
  );
}
