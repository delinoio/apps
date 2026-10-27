// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "@lynx-js/react";
import { AppIcon, MiniApp, SettingsSection } from "./AppNavigation";
import { Button } from "./components";
import { TabKind, type WorkspaceReport, type WorkspaceTab } from "./tabs";

export function TabBar({
  tabs,
  selected,
  reports,
  select,
  close,
  create,
  openSettings,
  fullTitles = false,
}: {
  fullTitles?: boolean;
  tabs: WorkspaceTab[];
  selected: string;
  reports: Record<string, WorkspaceReport>;
  select: (id: string) => void;
  close: (id: string) => void;
  create: () => void;
  openSettings: (section: SettingsSection) => void;
}) {
  const [height, setHeight] = useState(720);
  const [focused, setFocused] = useState(selected);
  useEffect(() => {
    "background only";
    setFocused(selected);
    if (!NativeModules.LaneOnline)
      lynx
        .createSelectorQuery()
        .select(`#tab-${selected}`)
        .invoke({ method: "setFocus", params: { focus: true } })
        .exec();
  }, [selected, tabs]);
  const index = tabs.findIndex((tab) => tab.id === selected);
  const moveFocus = (id: string, key: string) => {
    "background only";
    const current = tabs.findIndex((tab) => tab.id === id);
    const next =
      key === "Home"
        ? 0
        : key === "End"
          ? tabs.length - 1
          : (current + (key === "ArrowUp" ? -1 : 1) + tabs.length) %
            tabs.length;
    const target = tabs[next];
    if (!target) return;
    setFocused(target.id);
    lynx
      .createSelectorQuery()
      .select(`#tab-${target.id}`)
      .invoke({ method: "setFocus", params: { focus: true } })
      .exec();
  };
  // The outer scroller handles short windows; the minimum keeps tab rows
  // reachable between the header and utility actions.
  return (
    <view
      className="tab-sidebar"
      bindlayoutchange={(event) => setHeight(event.detail.height)}
    >
      <scroll-view className="tab-sidebar-scroll" scroll-orientation="vertical">
        <view
          className="tab-bar"
          style={{ height: `${Math.max(height, 360)}px` }}
        >
          <view className="tab-heading">
            <text className="heading">Tabs</text>
            <Button primary label="New tab" onClick={create}>
              + New tab
            </Button>
          </view>
          <scroll-view
            className="tab-scroll"
            accessibility-label="Workspace tabs"
            scroll-orientation="vertical"
            scroll-to-index={Math.max(0, index)}
          >
            {tabs.map((tab) => {
              const report = reports[tab.id];
              const title = report?.title || tab.target.label;
              const active = tab.id === selected;
              return (
                <view
                  className={`workspace-tab ${active ? "selected" : ""}`}
                  key={tab.id}
                >
                  <view
                    id={`tab-${tab.id}`}
                    className="button tab-select"
                    data-workspace-tab={tab.id}
                    data-tab-selected={String(active)}
                    data-tab-focus={String(tab.id === focused)}
                    accessibility-element
                    accessibility-traits="button"
                    accessibility-label={`${title}${report?.dirty ? ", Unsaved changes" : ""}`}
                    accessibility-value={active ? "Selected" : ""}
                    focusable={tab.id === focused}
                    bindtap={() => select(tab.id)}
                    bindkeydown={(event) => {
                      if (NativeModules.LaneOnline) return;
                      if (
                        ["ArrowUp", "ArrowDown", "Home", "End"].includes(
                          event.key,
                        )
                      )
                        moveFocus(tab.id, event.key);
                      else if (event.key === "Enter" || event.key === " ")
                        select(tab.id);
                    }}
                  >
                    {tab.target.kind === TabKind.NewTab ? (
                      <text className="tab-symbol">+</text>
                    ) : (
                      <AppIcon app={tab.target.app ?? MiniApp.Lane} />
                    )}
                    <text
                      className="tab-title"
                      text-maxline={fullTitles ? undefined : "1"}
                    >
                      {`${title}${fullTitles && report?.dirty ? " · Unsaved changes" : ""}`}
                    </text>
                    {report?.dirty && (
                      <view
                        className="tab-dirty"
                        accessibility-elements-hidden
                      />
                    )}
                    <view
                      className="tab-tooltip"
                      accessibility-elements-hidden
                      user-interaction-enabled={false}
                    >
                      <text>
                        {title}
                        {report?.dirty ? " · Unsaved changes" : ""}
                      </text>
                    </view>
                  </view>
                  <Button
                    label={`Close ${title}`}
                    disabled={report?.busy}
                    onClick={() => close(tab.id)}
                  >
                    ×
                  </Button>
                </view>
              );
            })}
          </scroll-view>
          <view className="tab-actions">
            <Button
              label="Account & sync"
              onClick={() => openSettings(SettingsSection.Account)}
            >
              Account & sync
            </Button>
            <Button
              label="App settings"
              onClick={() => openSettings(SettingsSection.Notifications)}
            >
              App settings
            </Button>
          </view>
        </view>
      </scroll-view>
    </view>
  );
}
