// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "@lynx-js/react";
import { Button } from "./components";
import { TabKind, type WorkspaceReport, type WorkspaceTab } from "./tabs";

export function TabBar({
  tabs,
  selected,
  reports,
  select,
  close,
  create,
  showAll,
}: {
  tabs: WorkspaceTab[];
  selected: string;
  reports: Record<string, WorkspaceReport>;
  select: (id: string) => void;
  close: (id: string) => void;
  create: () => void;
  showAll: () => void;
}) {
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
          : (current + (key === "ArrowLeft" ? -1 : 1) + tabs.length) %
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
  return (
    <view className="tab-bar">
      <scroll-view
        className="tab-scroll"
        accessibility-label="Workspace tabs"
        scroll-orientation="horizontal"
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
                    ["ArrowLeft", "ArrowRight", "Home", "End"].includes(
                      event.key,
                    )
                  )
                    moveFocus(tab.id, event.key);
                  else if (event.key === "Enter" || event.key === " ")
                    select(tab.id);
                }}
              >
                <svg
                  className="tab-symbol"
                  accessibility-elements-hidden
                  content={`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#68758b" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">${tab.target.kind === TabKind.Task || tab.target.kind === TabKind.NewTask ? '<rect x="4" y="4" width="16" height="16" rx="3"/><path d="m8 12 3 3 5-6"/>' : '<path d="M8 6h12M8 12h12M8 18h12M3 6h1M3 12h1M3 18h1"/>'}</svg>`}
                />
                <text className="tab-title" text-maxline="1">
                  {title}
                </text>
                {report?.dirty && (
                  <view className="tab-dirty" accessibility-elements-hidden />
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
        <Button label="New tab" onClick={create}>
          +
        </Button>
        <Button
          label={`Open tabs (${tabs.length})`}
          onClick={showAll}
        >{`⌄ ${tabs.length}`}</Button>
      </view>
    </view>
  );
}
