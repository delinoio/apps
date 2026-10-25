// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "@lynx-js/react";
import type { EntityResult } from "@delino/protos/lane/v1/lane_pb";
import type { UI } from "./serializable";
import type { LaneAdapter } from "./adapter";
import { Button, Dialog, Field, Notice } from "./components";
import { captureModes, realqa, type CaptureMode } from "./realqa-host";
import type { RealQAController } from "./useRealQA";
import { URLConnections } from "./URLConnections";
import { hostAction } from "./runtime";
export function CaptureActions({
  controller,
}: {
  controller: RealQAController;
}) {
  return (
    <view className="row wrap capture-actions">
      {captureModes.map(([mode, label]) => (
        <Button
          key={mode}
          disabled={controller.busy}
          onClick={() => void controller.capture(mode)}
        >
          {label}
        </Button>
      ))}
    </view>
  );
}
export function RealQA({
  controller,
  adapter,
  repositories,
  back,
  dirty,
}: {
  controller: RealQAController;
  adapter?: LaneAdapter;
  repositories: UI<EntityResult>[];
  back: () => void;
  dirty: (value: boolean) => void;
}) {
  const [shortcut, setShortcut] = useState<{
    mode: CaptureMode;
    value: string;
  }>();
  const [urlDirty, setUrlDirty] = useState(false);
  useEffect(() => {
    dirty(urlDirty || !!shortcut);
    return () => dirty(false);
  }, [dirty, urlDirty, shortcut]);
  const [disconnect, setDisconnect] = useState<string>();
  const { state, busy, run } = controller;
  const chrome = state?.chrome;
  if (!controller.available)
    return (
      <view className="coming-soon">
        <text className="heading">RealQA</text>
        <text>Capture on desktop. View saved captures in Lane.</text>
        <Button primary onClick={back}>
          Open Lane
        </Button>
      </view>
    );
  return (
    <scroll-view className="page-scroll" scroll-orientation="vertical">
      <view className="section realqa-home">
        <text className="heading" accessibility-heading>
          RealQA
        </text>
        <text>
          Capture a problem, annotate it, and save it as a Lane task. Saved
          captures appear in Lane tasks.
        </text>
        <Button onClick={back}>Open Lane</Button>
        <CaptureActions controller={controller} />
        {controller.error && <Notice>{controller.error}</Notice>}
        {state?.permission && (
          <text>Screen capture permission: {state.permission}</text>
        )}
        <Button
          onClick={() =>
            void run(() => realqa({ op: "openPermissionSettings" }))
          }
        >
          Open capture permission settings
        </Button>
        <view className="section">
          <text className="heading" accessibility-heading>
            Keyboard shortcuts
          </text>
          <text>
            Shortcuts work while Lane is running. Each shortcut is local to this
            device.
          </text>
          {captureModes.map(([mode, label]) => (
            <view className="card" key={mode}>
              <text>
                {label}: {state?.shortcuts[mode] || "Not set"}
              </text>
              <view className="row wrap">
                <Button
                  disabled={busy}
                  onClick={() =>
                    setShortcut({ mode, value: state?.shortcuts[mode] || "" })
                  }
                >
                  {state?.shortcuts[mode] ? "Change" : "Assign"}
                </Button>
                {state?.shortcuts[mode] && (
                  <Button
                    disabled={busy}
                    onClick={() =>
                      void run(() =>
                        realqa({ op: "shortcut", mode, accelerator: "" }),
                      )
                    }
                  >
                    Clear
                  </Button>
                )}
              </view>
            </view>
          ))}
        </view>
        {adapter ? (
          <URLConnections
            adapter={adapter}
            repositories={repositories}
            dirty={setUrlDirty}
          />
        ) : (
          <Notice>Open Lane storage to manage URL connections.</Notice>
        )}
        <view className="section">
          <text className="heading" accessibility-heading>
            Chrome connection
          </text>
          <text>{chrome?.state || "Disconnected"}</text>
          {chrome?.message && <Notice>{chrome.message}</Notice>}
          {chrome?.profile && <text>Profile: {chrome.profile}</text>}
          {chrome?.extensionId && <text>Extension: {chrome.extensionId}</text>}
          {chrome?.pending?.map((pending) => (
            <view className="card" key={pending.id}>
              <text>Approval pending · {pending.profileName}</text>
              <text>Extension: {pending.extensionId}</text>
              <view className="row wrap">
                <Button
                  disabled={busy}
                  onClick={() =>
                    void run(() =>
                      realqa({ op: "chrome.approve", id: pending.id }),
                    )
                  }
                >
                  Approve
                </Button>
                <Button
                  disabled={busy}
                  onClick={() =>
                    void run(() =>
                      realqa({ op: "chrome.reject", id: pending.id }),
                    )
                  }
                >
                  Reject
                </Button>
              </view>
            </view>
          ))}
          {chrome?.connections?.map((connection) => (
            <view className="card" key={connection.profileId}>
              <text>
                {connection.profileName} ·{" "}
                {connection.connected ? "Connected" : "Disconnected"}
              </text>
              <text>Extension: {connection.extensionId}</text>
              <Button
                disabled={busy}
                onClick={() => setDisconnect(connection.profileId)}
              >
                Disconnect
              </Button>
            </view>
          ))}
          {chrome?.listingUrl ? (
            <Button
              onClick={() =>
                void run(() => hostAction("openURL", chrome.listingUrl!))
              }
            >
              Install extension
            </Button>
          ) : (
            <text>
              The Chrome Web Store listing is not available in this build.
            </text>
          )}
          <text>
            Chrome captures only the visible portion of the selected element.
            Scroll and capture again for other portions.
          </text>
        </view>
        {shortcut && (
          <Dialog
            title="Assign keyboard shortcut"
            onClose={() => setShortcut(undefined)}
          >
            <Field
              label="Shortcut (for example Command+Shift+S or Ctrl+Shift+S)"
              focus
              value={shortcut.value}
              onChange={(value) => setShortcut({ ...shortcut, value })}
            />
            <text>
              If the shortcut is unavailable, the previous binding is retained.
            </text>
            {controller.error && <Notice>{controller.error}</Notice>}
            <Button onClick={() => setShortcut(undefined)}>Cancel</Button>
            <Button
              primary
              disabled={busy || !shortcut.value.trim()}
              onClick={() =>
                void run(async () => {
                  await realqa({
                    op: "shortcut",
                    mode: shortcut.mode,
                    accelerator: shortcut.value,
                  });
                  setShortcut(undefined);
                })
              }
            >
              Save shortcut
            </Button>
          </Dialog>
        )}
        {disconnect && (
          <Dialog
            title="Disconnect Chrome?"
            onClose={() => setDisconnect(undefined)}
          >
            <text>
              This revokes this profile's permission. New captures require
              approval again.
            </text>
            <Button onClick={() => setDisconnect(undefined)}>Cancel</Button>
            <Button
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  await realqa({
                    op: "chrome.disconnect",
                    profileId: disconnect,
                  });
                  setDisconnect(undefined);
                })
              }
            >
              Disconnect Chrome
            </Button>
          </Dialog>
        )}
      </view>
    </scroll-view>
  );
}
export function RealQADialogs({
  controller,
  onOpenSaved,
}: {
  controller: RealQAController;
  onOpenSaved?: (saved: { taskId: string; spaceId?: string }) => void;
}) {
  const { sources, busy, run } = controller;
  const pending =
    controller.state?.window !== "composer"
      ? controller.state?.chrome?.pending?.[0]
      : undefined;
  return (
    <>
      {pending && (
        <Dialog
          title="Approve Chrome connection?"
          onClose={() =>
            void run(() => realqa({ op: "chrome.reject", id: pending.id }))
          }
        >
          <text>Profile: {pending.profileName}</text>
          <text>Extension: {pending.extensionId}</text>
          <Notice>
            Allow this Chrome profile to send explicit captures to Lane on this
            device.
          </Notice>
          <Button
            primary
            disabled={busy}
            onClick={() =>
              void run(() => realqa({ op: "chrome.approve", id: pending.id }))
            }
          >
            Approve
          </Button>
          <Button
            disabled={busy}
            onClick={() =>
              void run(() => realqa({ op: "chrome.reject", id: pending.id }))
            }
          >
            Reject
          </Button>
          {controller.error && <Notice>{controller.error}</Notice>}
        </Dialog>
      )}
      {sources && (
        <Dialog
          title={
            sources.mode === "screen" ? "Choose a monitor" : "Choose a window"
          }
          onClose={() => controller.setSources(undefined)}
        >
          {!sources.sources.length && (
            <Notice>
              No capture sources are available. Check screen capture permissions
              and retry.
            </Notice>
          )}
          {sources.sources.map((source) => (
            <Button
              key={source.id}
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  controller.setSources(undefined);
                  await realqa({
                    op: "capture",
                    mode: sources.mode,
                    sourceId: source.id,
                  });
                })
              }
            >{`${source.name} · ${source.width} × ${source.height}`}</Button>
          ))}
          <Button onClick={() => controller.setSources(undefined)}>
            Cancel selection
          </Button>
        </Dialog>
      )}
      {controller.discard && (
        <Dialog
          title="Discard unsaved captures?"
          onClose={() => controller.setDiscard(false)}
        >
          <Notice>
            The unsaved captures and annotations will be removed. They cannot be
            recovered.
          </Notice>
          <Button primary onClick={() => controller.setDiscard(false)}>
            Keep editing
          </Button>
          <Button
            disabled={busy}
            onClick={() =>
              void run(async () => {
                await realqa({ op: "discard" });
                controller.setDiscard(false);
              })
            }
          >
            Discard draft
          </Button>
        </Dialog>
      )}
      {controller.saved && (
        <Dialog
          title="Saved to Lane"
          onClose={() => controller.setSaved(undefined)}
        >
          <Notice>
            The task is saved locally. Optional synchronization may still be
            pending.
          </Notice>
          <Button
            primary
            onClick={() =>
              void run(async () => {
                onOpenSaved?.(controller.saved!);
                controller.setSaved(undefined);
              })
            }
          >
            Open in Lane
          </Button>
        </Dialog>
      )}
    </>
  );
}
