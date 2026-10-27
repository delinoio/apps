// SPDX-License-Identifier: Apache-2.0
import type { UI } from "./serializable";
import { useEffect, useRef, useState } from "@lynx-js/react";
import {
  AccountService,
  DataDeletionMode,
  LocalRetention,
  NotificationContent,
  type Device as ProtoDevice,
  type Connection as ProtoConnection,
} from "@repo/protos/lane/v1/lane_pb";
import {
  meta,
  type LaneAdapter,
  type LocalStatus,
  type NotificationPreferences,
} from "./adapter";
import { Button, Choices, Dialog, Field, Notice } from "./components";
import {
  reconnectTarget,
  confirmReconnect,
  type ReconnectTarget,
} from "./reconnect";
import { hostAction, listenReauthentication } from "./runtime";
import { AppUpdates } from "./AppUpdates";
import { ResetDevelopmentDialog } from "./ResetDevelopment";
import { describeItemError } from "./planning";
import { SettingsSection } from "./AppNavigation";
// Native hosts return ProtoJSON enum names while the web adapter and Choices use
// numbers. Display and Save share this conversion so a save never changes the
// content preference that the screen shows. Missing or unspecified values keep
// the DETAIL default.
const notificationContent = (
  value: NotificationPreferences["content"] | undefined,
): NotificationContent =>
  Number(value) === NotificationContent.GENERIC ||
  value === "NOTIFICATION_CONTENT_GENERIC"
    ? NotificationContent.GENERIC
    : NotificationContent.DETAIL;
const time = (value?: { seconds: string }) =>
  value ? new Date(Number(value.seconds) * 1000).toLocaleString() : "Never";
export function Settings({
  adapter,
  onReset,
  target,
  stateChanged,
  active: visible = true,
  beforeDataChange,
}: {
  adapter: LaneAdapter;
  onReset: () => void;
  active?: boolean;
  beforeDataChange?: () => Promise<boolean>;
  stateChanged?: (dirty: boolean, busy: boolean, exitBusy: boolean) => void;
  target?: { section: SettingsSection; request: number };
}) {
  const revealedTarget = useRef<number | undefined>(undefined);
  const [deletion, setDeletion] = useState<{
    challengeId: string;
    ready: boolean;
    mode: DataDeletionMode;
    account: string;
  }>();
  const deletionRef = useRef(deletion);
  deletionRef.current = deletion;
  const cancelDeletion = () => {
    const pending = deletionRef.current;
    deletionRef.current = undefined;
    setDeletion(undefined);
    adapter.cancelDeletion?.();
    if (pending)
      void hostAction("cancelReauthentication", pending.challengeId).catch(
        (e) => setError(e.message),
      );
  };
  useEffect(() => {
    if (adapter.mode !== "native") return;
    const stop = listenReauthentication((challengeId) => {
      if (deletionRef.current?.challengeId === challengeId)
        setDeletion({ ...deletionRef.current, ready: true });
    });
    return () => {
      stop();
      const pending = deletionRef.current;
      adapter.cancelDeletion?.();
      if (pending)
        void hostAction("cancelReauthentication", pending.challengeId).catch(
          () => {},
        );
    };
  }, [adapter]);
  const [status, setStatus] = useState<LocalStatus>();
  const prefsDirty = useRef(false);
  const [prefs, setPrefs] = useState<NotificationPreferences>();
  const [devices, setDevices] = useState<Device[]>([]);
  const [connections, setConnections] = useState<Connection[]>([]);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const [operationBusy, setBusy] = useState(false);
  const [updateBusy, setUpdateBusy] = useState(false);
  const busy = operationBusy || updateBusy;
  useEffect(() => {
    stateChanged?.(prefsDirty.current, busy, operationBusy);
  }, [prefs, busy, operationBusy]);
  const [confirm, setConfirm] = useState<
    null | "disconnect" | "local" | "server" | "everywhere" | "reconnect"
  >(null);
  const [reconnect, setReconnect] = useState<ReconnectTarget>();
  const [resetOpen, setResetOpen] = useState(false);
  const [account, setAccount] = useState("");
  const [retention, setRetention] = useState<LocalRetention>(
    LocalRetention.KEEP_LOCAL,
  );
  const run = async (fn: () => Promise<void>, changesData = false) => {
    if (changesData && beforeDataChange && !(await beforeDataChange())) return;
    setBusy(true);
    setError("");
    setMessage("");
    try {
      await fn();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Operation failed.");
    } finally {
      setBusy(false);
    }
  };
  const load = async () => {
    const local = await adapter.local("GetLocalStatus", {});
    setStatus(local);
    const latestPreferences = await adapter.local(
      "GetNotificationPreferences",
      {},
    );
    if (!prefsDirty.current) setPrefs(latestPreferences);
    // Account lists are online-only; local preferences stay usable if these fail.
    const ds: Device[] = [];
    let cursor = "";
    do {
      const r = await adapter.call(AccountService.method.listDevices, {
        page: { size: 100, cursor },
      });
      ds.push(...r.items);
      cursor = r.nextCursor;
    } while (cursor);
    setDevices(ds);
    const cs: Connection[] = [];
    cursor = "";
    do {
      const r = await adapter.call(AccountService.method.listConnections, {
        page: { size: 100, cursor },
      });
      cs.push(...r.items);
      cursor = r.nextCursor;
    } while (cursor);
    setConnections(cs);
  };
  useEffect(() => {
    void run(load);
  }, [adapter]);
  useEffect(() => {
    "background only";
    if (
      !visible ||
      !target ||
      adapter.mode === "web" ||
      revealedTarget.current === target.request
    )
      return;
    revealedTarget.current = target.request;
    // Wait for layout before measuring; native acceptance is tracked per host.
    let active = true;
    const timer = setTimeout(() => {
      const query = lynx.createSelectorQuery();
      query
        .select(`#settings-${target.section}`)
        .invoke({
          method: "boundingClientRect",
          success: (heading) => {
            if (!active) return;
            lynx
              .createSelectorQuery()
              .select("#settings-scroll")
              .invoke({
                method: "boundingClientRect",
                success: (viewport) => {
                  if (!active) return;
                  lynx
                    .createSelectorQuery()
                    .select("#settings-scroll")
                    .invoke({
                      method: "getScrollInfo",
                      success: (info) => {
                        if (!active) return;
                        lynx
                          .createSelectorQuery()
                          .select("#settings-scroll")
                          .invoke({
                            method: "scrollTo",
                            params: {
                              offset: Math.max(
                                0,
                                info.scrollY + heading.top - viewport.top,
                              ),
                              smooth: false,
                            },
                          })
                          .exec();
                        lynx
                          .createSelectorQuery()
                          .select(`#settings-${target.section}`)
                          .invoke({
                            method: "setFocus",
                            params: { focus: true },
                          })
                          .exec();
                        lynx
                          .createSelectorQuery()
                          .select(`#settings-${target.section}`)
                          .invoke({ method: "requestAccessibilityFocus" })
                          .exec();
                      },
                    })
                    .exec();
                },
              })
              .exec();
          },
        })
        .exec();
    }, 0);
    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, [target, adapter, visible]);
  return (
    <view
      className="section"
      data-settings-target={
        visible && target ? `${target.section}:${target.request}` : ""
      }
    >
      <text className="heading">Settings</text>
      {visible && (
        <AppUpdates
          settingsDirty={() => prefsDirty.current}
          settingsBusy={operationBusy}
          onBusyChange={setUpdateBusy}
        />
      )}
      <Button onClick={() => void run(load)}>Refresh settings</Button>
      {error && <Notice>{error}</Notice>}
      {message && <Notice>{message}</Notice>}
      <view className="card">
        <view
          id="settings-account"
          accessibility-element
          accessibility-heading
          focusable
          accessibility-label={
            adapter.mode === "native"
              ? "Local data and synchronization"
              : "Online account"
          }
        >
          <text className="heading" accessibility-elements-hidden>
            {adapter.mode === "native"
              ? "Local data and synchronization"
              : "Online account"}
          </text>
        </view>
        <text>
          {status?.syncEnabled
            ? "Synchronization enabled"
            : adapter.mode === "native"
              ? "Local data on this device"
              : "Web requires an online connection"}
        </text>
        <text>
          {status?.connected ? "Connected" : "Disconnected"} · Pending items:{" "}
          {status?.pendingItemCount ?? "Unknown"}
        </text>
        {status?.reconnectRequired && (
          <Notice>
            Reconnection required. Synchronization is stopped. Retained local
            data uploads only after you confirm a new connection.
          </Notice>
        )}
        {status?.syncErrorReason && <Notice>{status.syncErrorReason}</Notice>}
        <text>Last synchronization: {status?.lastSyncAt || "Never"}</text>
        {status?.itemErrors?.map((e) => (
          <Notice key={e.entityId}>
            {e.entityId}: {describeItemError(e.reason)} Other eligible items can
            continue synchronizing.
          </Notice>
        ))}
        {adapter.mode === "native" && !!status?.itemErrors?.length && (
          <Button
            disabled={busy}
            onClick={() =>
              void run(async () => {
                await adapter.local("RetryBlockedItems", {});
                await load();
                setMessage(
                  "Blocked items were returned to pending and will synchronize again.",
                );
              })
            }
          >
            Retry blocked items
          </Button>
        )}
        <view className="row wrap">
          <Button
            disabled={busy}
            onClick={() => {
              if (status?.reconnectRequired) {
                try {
                  setReconnect(reconnectTarget(status));
                  setConfirm("reconnect");
                } catch (e) {
                  setError((e as Error).message);
                }
                return;
              }
              void run(async () => {
                const r = await adapter.local("ConnectSync", { accountId: "" });
                if (adapter.mode === "web" && r.authorizationUrl)
                  await hostAction("openURL", r.authorizationUrl);
                setMessage(
                  "Complete browser approval, then refresh. Retained data from other accounts stays separate.",
                );
              }, true);
            }}
          >
            {adapter.mode === "native"
              ? "Connect synchronization / switch account"
              : "Sign in / switch account"}
          </Button>
          <Button disabled={busy} onClick={() => setConfirm("disconnect")}>
            Log out / disable synchronization
          </Button>
        </view>
      </view>
      <view className="card">
        <view
          id="settings-notifications"
          accessibility-element
          accessibility-heading
          focusable
          accessibility-label="Notifications on this device"
        >
          <text className="heading" accessibility-elements-hidden>
            Notifications on this device
          </text>
        </view>
        <text>
          Only new proposals created through MCP notify you. In-app proposal
          counts remain available.
        </text>
        {prefs && (
          <>
            <Choices
              disabled={busy}
              label="Enable notifications"
              value={prefs.enabled ? "on" : "off"}
              options={[
                ["off", "Off"],
                ["on", "On"],
              ]}
              onChange={(v) => {
                prefsDirty.current = true;
                setPrefs({ ...prefs, enabled: v === "on" });
              }}
            />
            <Choices
              disabled={busy}
              label="Notification content"
              value={notificationContent(prefs.content)}
              options={[
                [NotificationContent.DETAIL, "Repository and title"],
                [NotificationContent.GENERIC, "Generic notice"],
              ]}
              onChange={(content) => {
                prefsDirty.current = true;
                setPrefs({ ...prefs, content });
              }}
            />
            <text>Permission: {String(prefs.permission)}</text>
            <Button
              disabled={busy}
              primary
              onClick={() =>
                void run(async () => {
                  const updated = await adapter.local(
                    "SetNotificationPreferences",
                    {
                      enabled: prefs.enabled,
                      content: notificationContent(prefs.content),
                    },
                  );
                  prefsDirty.current = false;
                  setPrefs(updated);
                  setMessage(
                    "Notification preferences saved. Check the permission status.",
                  );
                })
              }
            >
              Save notification preferences
            </Button>
          </>
        )}
        <text className="muted">
          Windows remote push is unavailable. Android without required Google
          Play services and other unsupported environments retain available
          local notifications and in-app indications.
        </text>
      </view>
      <view className="card">
        <text className="heading">Devices</text>
        <text>
          Disconnection blocks later server access. It cannot erase an offline
          device’s existing data.
        </text>
        {devices.map((d) => (
          <view className="card" key={d.id}>
            <text>{d.name}</text>
            <text>
              Last synchronization: {time(d.lastSyncAt)} ·{" "}
              {d.disconnectedAt ? "Disconnected" : "Connected"}
            </text>
            <Button
              disabled={busy || !!d.disconnectedAt}
              onClick={() =>
                void run(async () => {
                  await adapter.call(AccountService.method.disconnectDevice, {
                    meta: meta(),
                    deviceId: d.id,
                  });
                  await load();
                })
              }
            >
              Disconnect device
            </Button>
          </view>
        ))}
      </view>
      <view className="card">
        <text className="heading">Approved remote MCP connections</text>
        <Notice>
          Each approved connection has full management access to every
          repository in this account.
        </Notice>
        {connections.map((c) => (
          <view className="card" key={c.id}>
            <text>
              {c.name} · Approved {time(c.approvedAt)}
            </text>
            <text>{c.revokedAt ? "Revoked" : "Full management access"}</text>
            <Button
              disabled={busy || !!c.revokedAt}
              onClick={() =>
                void run(async () => {
                  await adapter.call(AccountService.method.revokeConnection, {
                    meta: meta(),
                    connectionId: c.id,
                  });
                  await load();
                })
              }
            >
              Revoke access
            </Button>
          </view>
        ))}
      </view>
      {adapter.mode === "native" && (
        <view className="card">
          <text className="heading">Development data</text>
          <text>
            Lane is pre-release. If a data space cannot be opened because its
            format is outdated, or you want to discard disposable development
            data, reset this space explicitly. Lane never resets data
            automatically.
          </text>
          <Button
            onClick={() => {
              setError("");
              setResetOpen(true);
            }}
          >
            Reset development data
          </Button>
        </view>
      )}
      <view className="card">
        <text className="heading">Delete Lane data</text>
        <text>These actions preserve your shared Logto identity.</text>
        {adapter.mode === "native" && (
          <Button onClick={() => setConfirm("local")}>
            Remove only this device’s data
          </Button>
        )}
        <Button
          onClick={() => {
            cancelDeletion();
            setConfirm("everywhere");
          }}
        >
          Delete server and connected-device data
        </Button>
        <Button
          onClick={() => {
            cancelDeletion();
            setConfirm("server");
          }}
        >
          Delete server data, keep local data
        </Button>
      </view>
      {resetOpen && (
        <ResetDevelopmentDialog
          onClose={() => setResetOpen(false)}
          onConfirm={async () => {
            if (beforeDataChange && !(await beforeDataChange()))
              throw new Error("Data reset cancelled. Your input is retained.");
            await adapter.local("ResetDevelopmentData", { confirmed: true });
            setResetOpen(false);
            onReset();
          }}
        />
      )}
      {deletion && (
        <Dialog title="Complete data deletion" onClose={cancelDeletion}>
          <Notice>
            {deletion.ready
              ? "Reauthentication completed. Select Delete now to permanently remove the confirmed data."
              : "Complete reauthentication in your browser. No data is deleted until you return and select Delete now."}
          </Notice>
          <text>Account: {deletion.account}</text>
          <text>
            {deletion.mode === DataDeletionMode.CLEAR_SERVER_KEEP_LOCAL
              ? "Delete server data, keep local data"
              : "Delete server and connected-device data"}
          </text>
          {error && <Notice>{error}</Notice>}
          <Button
            disabled={busy || !deletion.ready}
            primary
            onClick={() =>
              void run(async () => {
                if (!adapter.deleteReauthenticated)
                  throw new Error(
                    "This host does not support secure deletion.",
                  );
                await adapter.deleteReauthenticated(deletion.challengeId);
                deletionRef.current = undefined;
                setDeletion(undefined);
                onReset();
              }, true)
            }
          >
            Delete now
          </Button>
          <Button onClick={cancelDeletion}>Cancel deletion</Button>
        </Dialog>
      )}
      {confirm && (
        <Dialog
          title={
            confirm === "reconnect"
              ? "Reconnect retained data"
              : confirm === "disconnect"
                ? "Disconnect this account"
                : "Confirm permanent data removal"
          }
          onClose={() => setConfirm(null)}
        >
          {confirm === "reconnect" ? (
            <Notice>
              Reconnect this account and upload retained local data into the new
              server dataset. All retained repositories will synchronize after
              fresh browser approval. Automatic retries do not reconnect.
            </Notice>
          ) : confirm === "disconnect" ? (
            <>
              <Notice>
                Choose whether to retain local data in this account’s separate
                space.
              </Notice>
              <Choices
                disabled={busy}
                label="Local data"
                value={retention}
                options={[
                  [LocalRetention.KEEP_LOCAL, "Keep local data"],
                  [LocalRetention.REMOVE_LOCAL, "Remove local data"],
                ]}
                onChange={setRetention}
              />
            </>
          ) : confirm === "local" ? (
            <Notice>
              Remove content only from this device. This does not delete server
              data. There is no recovery of unsynchronized content.
            </Notice>
          ) : (
            <>
              <Notice>
                {confirm === "server"
                  ? "Delete server synchronization data and webhook receiving settings. All devices and grants disconnect. Retained local data requires explicit reconnection before uploading again."
                  : "Permanently delete server content and content on connected devices. Offline devices receive a deletion directive on reconnect."}
              </Notice>
              <Field
                label="Confirm target account ID"
                value={account}
                onChange={setAccount}
                focus
              />
              <Notice>
                Continue to Logto reauthentication. The operation completes only
                after a fresh account-bound approval.
              </Notice>
            </>
          )}
          {error && <Notice>{error}</Notice>}
          <Button
            primary
            disabled={
              busy ||
              ((confirm === "server" || confirm === "everywhere") &&
                !account.trim())
            }
            onClick={() =>
              void run(async () => {
                if (confirm === "reconnect") {
                  if (!reconnect) throw new Error("Start a new reconnection.");
                  await confirmReconnect(adapter, reconnect);
                  setConfirm(null);
                  setMessage(
                    "Complete fresh browser approval to reconnect this account.",
                  );
                } else if (confirm === "disconnect") {
                  await adapter.local("DisconnectSync", { retention });
                  setConfirm(null);
                  onReset();
                } else if (confirm === "local") {
                  await adapter.local("RemoveDeviceData", {});
                  setConfirm(null);
                  onReset();
                } else {
                  const r = await adapter.call(
                    AccountService.method.beginDataDeletion,
                    {
                      meta: meta(),
                      mode:
                        confirm === "server"
                          ? DataDeletionMode.CLEAR_SERVER_KEEP_LOCAL
                          : DataDeletionMode.DELETE_EVERYWHERE,
                      confirmedAccountId: account,
                    },
                  );
                  if (adapter.mode === "native") {
                    const pending = {
                      challengeId: r.challengeId,
                      ready: false,
                      account,
                      mode:
                        confirm === "server"
                          ? DataDeletionMode.CLEAR_SERVER_KEEP_LOCAL
                          : DataDeletionMode.DELETE_EVERYWHERE,
                    };
                    deletionRef.current = pending;
                    setDeletion(pending);
                    setConfirm(null);
                  }
                  await hostAction("openURL", r.authorizationUrl);
                  setMessage(
                    "Complete Logto reauthentication in the browser. Your data has not yet been deleted.",
                  );
                  setConfirm(null);
                }
              }, true)
            }
          >
            {confirm === "server" || confirm === "everywhere"
              ? "Continue to reauthentication"
              : "Confirm"}
          </Button>
        </Dialog>
      )}
    </view>
  );
}

type Device = UI<ProtoDevice>;

type Connection = UI<ProtoConnection>;
