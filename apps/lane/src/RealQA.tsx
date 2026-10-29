// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "@lynx-js/react";
import type { EntityResult } from "@repo/protos/lane/v1/lane_pb";
import type { UI } from "./serializable";
import type { LaneAdapter } from "./adapter";
import { Button, Dialog, Field, Notice } from "./components";
import { captureModes, realqa, CaptureMode } from "./realqa-host";
import type { RealQAController } from "./useRealQA";
import { URLConnections } from "./URLConnections";
import { hostAction } from "./runtime";
export function CaptureActions({
  controller,
}: {
  controller: RealQAController;
}) {
  const [width, setWidth] = useState(0);
  const details = {
    [CaptureMode.Screen]: ["▣", "Choose a monitor"],
    [CaptureMode.Window]: ["▯", "Choose a window"],
    [CaptureMode.Region]: ["▱", "Select an area"],
    [CaptureMode.DOM]: ["⌗", "Capture a visible element"],
  };
  return (
    <view
      className={`capture-actions capture-columns-${width >= 940 ? 4 : width >= 460 ? 2 : 1}`}
      bindlayoutchange={(event) => setWidth(event.detail.width)}
    >
      {captureModes.map(([mode, label]) => {
        const activate = () => {
          if (!controller.busy) void controller.capture(mode);
        };
        return (
          <view
            key={mode}
            className={`button realqa-capture-card ${controller.busy ? "disabled" : ""}`}
            accessibility-element
            accessibility-traits="button"
            accessibility-label={`${label}. ${details[mode][1]}`}
            accessibility-value={controller.busy ? "Disabled" : undefined}
            focusable={!controller.busy}
            bindtap={activate}
            bindkeydown={(event) => {
              if (event.key === "Enter" || event.key === " ") activate();
            }}
          >
            <text className="realqa-capture-icon" accessibility-elements-hidden>
              {details[mode][0]}
            </text>
            <text className="realqa-capture-label">{label}</text>
            <text className="muted">{details[mode][1]}</text>
          </view>
        );
      })}
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
  const [narrow, setNarrow] = useState(false);
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
      <view
        className={`realqa-home ${narrow ? "compact" : ""}`}
        bindlayoutchange={(event) => setNarrow(event.detail.width < 900)}
      >
        <view className="realqa-header">
          <view className="realqa-header-copy">
            <text className="heading" accessibility-heading>
              RealQA
            </text>
            <text>
              Capture a problem, annotate it, and save it as a Lane task.
            </text>
            <text className="muted">Saved captures appear in Lane tasks.</text>
          </view>
          <Button onClick={back}>Open Lane</Button>
        </view>
        <CaptureActions controller={controller} />
        <view className="realqa-permission-row">
          <text>
            Screen capture permission: {state?.permission || "Loading…"}
          </text>
          <Button
            disabled={busy}
            onClick={() =>
              void run(() => realqa({ op: "openPermissionSettings" }))
            }
          >
            Open capture permission settings
          </Button>
        </view>
        {controller.error && <Notice>{controller.error}</Notice>}
        <view className="realqa-section">
          <text className="heading" accessibility-heading>
            Keyboard shortcuts
          </text>
          <text>
            Shortcuts work while Lane is running. Each shortcut is local to this
            device.
          </text>
          <view className="realqa-group">
            {captureModes.map(([mode, label]) => (
              <view className="realqa-setting-row" key={mode}>
                <view className="realqa-row-value">
                  <text className="realqa-row-title">{label}</text>
                  <text className="realqa-shortcut-value">
                    {state ? state.shortcuts[mode] || "Not set" : "Loading…"}
                  </text>
                </view>
                <view className="realqa-row-actions">
                  <Button
                    disabled={busy || !state}
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
        </view>
        {adapter ? (
          <URLConnections
            adapter={adapter}
            repositories={repositories}
            dirty={setUrlDirty}
          />
        ) : (
          <view className="realqa-section">
            <text className="heading" accessibility-heading>
              URL connections
            </text>
            <Notice>Open Lane storage to manage URL connections.</Notice>
          </view>
        )}
        <view className="realqa-section">
          <text className="heading" accessibility-heading>
            Chrome connection
          </text>
          <text className="muted">Approvals are local to this device.</text>
          <text>{chrome?.state || (state ? "Disconnected" : "Loading…")}</text>
          {chrome?.message && <Notice>{chrome.message}</Notice>}
          {chrome?.profile && (
            <text className="realqa-long-value">Profile: {chrome.profile}</text>
          )}
          {chrome?.extensionId && (
            <text className="realqa-long-value">
              Extension: {chrome.extensionId}
            </text>
          )}
          {!!(chrome?.pending?.length || chrome?.connections?.length) && (
            <view className="realqa-group">
              {chrome?.pending?.map((pending) => (
                <view className="realqa-setting-row" key={pending.id}>
                  <view className="realqa-row-value">
                    <text className="realqa-long-value">
                      Approval pending · {pending.profileName}
                    </text>
                    <text className="realqa-long-value">
                      Extension: {pending.extensionId}
                    </text>
                  </view>
                  <view className="realqa-row-actions">
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
                <view className="realqa-setting-row" key={connection.profileId}>
                  <view className="realqa-row-value">
                    <text className="realqa-long-value">
                      {connection.profileName} ·{" "}
                      {connection.connected ? "Connected" : "Disconnected"}
                    </text>
                    <text className="realqa-long-value">
                      Extension: {connection.extensionId}
                    </text>
                  </view>
                  <Button
                    disabled={busy}
                    onClick={() => setDisconnect(connection.profileId)}
                  >
                    Disconnect
                  </Button>
                </view>
              ))}
            </view>
          )}
          {chrome?.listingUrl ? (
            <Button
              disabled={busy}
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
            className="realqa-dialog"
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
            className="realqa-dialog"
            title="Disconnect Chrome?"
            onClose={() => setDisconnect(undefined)}
          >
            <text>
              This revokes this profile's permission. New captures require
              approval again.
            </text>
            {controller.error && <Notice>{controller.error}</Notice>}
            <Button focusOnShow onClick={() => setDisconnect(undefined)}>
              Cancel
            </Button>
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
          className="realqa-dialog"
          title="Approve Chrome connection?"
          onClose={() =>
            void run(() => realqa({ op: "chrome.reject", id: pending.id }))
          }
        >
          <text className="realqa-long-value">
            Profile: {pending.profileName}
          </text>
          <text className="realqa-long-value">
            Extension: {pending.extensionId}
          </text>
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
            focusOnShow
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
          className="realqa-dialog"
          title={
            sources.mode === "screen" ? "Choose a monitor" : "Choose a window"
          }
          onClose={() => controller.setSources(undefined)}
        >
          {!sources.sources.length && (
            <>
              <Notice>
                No capture sources are available. Check screen capture
                permissions and retry.
              </Notice>
              <Button
                disabled={busy}
                onClick={() => {
                  controller.setSources(undefined);
                  void controller.capture(sources.mode);
                }}
              >
                Retry sources
              </Button>
            </>
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
          className="realqa-dialog"
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
          className="realqa-dialog"
          title="Saved to Lane"
          onClose={() => controller.setSaved(undefined)}
        >
          {controller.error && <Notice>{controller.error}</Notice>}
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
