// SPDX-License-Identifier: Apache-2.0
import type { UI } from "./serializable";
import { useEffect, useState } from "@lynx-js/react";
import {
  AccountService,
  DataDeletionMode,
  LocalRetention,
  NotificationContent,
  type Device as ProtoDevice,
  type Connection as ProtoConnection,
} from "@delino/protos/lane/v1/lane_pb";
import {
  meta,
  type LaneAdapter,
  type LocalStatus,
  type NotificationPreferences,
} from "./adapter";
import { Button, Choices, Dialog, Field, Notice } from "./components";
import { hostAction } from "./runtime";
const time = (value?: { seconds: string }) =>
  value ? new Date(Number(value.seconds) * 1000).toLocaleString() : "Never";
export function Settings({
  adapter,
  onReset,
}: {
  adapter: LaneAdapter;
  onReset: () => void;
}) {
  const [status, setStatus] = useState<LocalStatus>();
  const [prefs, setPrefs] = useState<NotificationPreferences>();
  const [devices, setDevices] = useState<Device[]>([]);
  const [connections, setConnections] = useState<Connection[]>([]);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState<
    null | "disconnect" | "local" | "server" | "everywhere"
  >(null);
  const [account, setAccount] = useState("");
  const [retention, setRetention] = useState<LocalRetention>(
    LocalRetention.KEEP_LOCAL,
  );
  const run = async (fn: () => Promise<void>) => {
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
    setPrefs(await adapter.local("GetNotificationPreferences", {}));
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
  return (
    <view className="section">
      <text className="heading">Settings</text>
      <Button onClick={() => void run(load)}>Refresh settings</Button>
      {error && <Notice>{error}</Notice>}
      {message && <Notice>{message}</Notice>}
      <view className="card">
        <text className="heading">
          {adapter.mode === "native"
            ? "Local data and synchronization"
            : "Online account"}
        </text>
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
        <text>Last synchronization: {status?.lastSyncAt || "Never"}</text>
        {status?.itemErrors?.map((e) => (
          <Notice key={e.entityId}>
            {e.entityId}: {e.reason}. Other eligible items can continue
            synchronizing.
          </Notice>
        ))}
        <view className="row wrap">
          <Button
            disabled={busy}
            onClick={() =>
              void run(async () => {
                const r = await adapter.local("ConnectSync", { accountId: "" });
                if (r.authorizationUrl)
                  await hostAction("openURL", r.authorizationUrl);
                setMessage(
                  "Complete browser approval, then refresh. Retained data from other accounts stays separate.",
                );
              })
            }
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
        <text className="heading">Notifications on this device</text>
        <text>
          Only new proposals created through MCP notify you. In-app proposal
          counts remain available.
        </text>
        {prefs && (
          <>
            <Choices
              label="Enable notifications"
              value={prefs.enabled ? "on" : "off"}
              options={[
                ["off", "Off"],
                ["on", "On"],
              ]}
              onChange={(v) => setPrefs({ ...prefs, enabled: v === "on" })}
            />
            <Choices
              label="Notification content"
              value={
                Number(prefs.content) ||
                (prefs.content === "NOTIFICATION_CONTENT_GENERIC" ? 2 : 1)
              }
              options={[
                [NotificationContent.DETAIL, "Repository and title"],
                [NotificationContent.GENERIC, "Generic notice"],
              ]}
              onChange={(content) => setPrefs({ ...prefs, content })}
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
                      content: Number(prefs.content) || 1,
                    },
                  );
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
      <view className="card">
        <text className="heading">Delete Lane data</text>
        <text>These actions preserve your shared Logto identity.</text>
        {adapter.mode === "native" && (
          <Button onClick={() => setConfirm("local")}>
            Remove only this device’s data
          </Button>
        )}
        <Button onClick={() => setConfirm("everywhere")}>
          Delete server and connected-device data
        </Button>
        <Button onClick={() => setConfirm("server")}>
          Delete server data, keep local data
        </Button>
      </view>
      {confirm && (
        <Dialog
          title={
            confirm === "disconnect"
              ? "Disconnect this account"
              : "Confirm permanent data removal"
          }
          onClose={() => setConfirm(null)}
        >
          {confirm === "disconnect" ? (
            <>
              <Notice>
                Choose whether to retain local data in this account’s separate
                space.
              </Notice>
              <Choices
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
                if (confirm === "disconnect") {
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
                  await hostAction("openURL", r.authorizationUrl);
                  setMessage(
                    "Complete Logto reauthentication in the browser. Your data has not yet been deleted.",
                  );
                  setConfirm(null);
                }
              })
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
