// SPDX-License-Identifier: Apache-2.0
import { useState } from "@lynx-js/react";
import { Button } from "./components";
import realqaIcon from "../assets/realqa-icon.png?inline";
import laneIcon from "../assets/icon-source.png?inline";
import paceIcon from "../assets/pace-icon.png?inline";

export enum MiniApp {
  Lane = "Lane",
  Pace = "Pace",
  RealQA = "RealQA",
}

export enum SettingsSection {
  Account = "account",
  Notifications = "notifications",
}

const utilityIcons = {
  [SettingsSection.Account]:
    '<circle cx="12" cy="8" r="4"/><path d="M4 21v-2a8 8 0 0 1 16 0v2"/>',
  [SettingsSection.Notifications]:
    '<path d="m9 3-1 3-3 1 1 3-2 2 2 2-1 3 3 1 1 3h6l1-3 3-1-1-3 2-2-2-2 1-3-3-1-1-3Z"/><circle cx="12" cy="12" r="3"/>',
};

// Inline the artwork so packaged native hosts do not need a separate image URL.
export function AppIcon({ app }: { app: MiniApp }) {
  return (
    <view
      className={`app-icon icon-${app.toLowerCase()}`}
      accessibility-elements-hidden
    >
      {app === MiniApp.Lane ? (
        <image className="lane-icon" src={laneIcon} mode="aspectFit" />
      ) : app === MiniApp.Pace ? (
        <image className="pace-artwork" src={paceIcon} mode="aspectFit" />
      ) : (
        <image className="realqa-artwork" src={realqaIcon} mode="aspectFit" />
      )}
    </view>
  );
}

export function AppRail({
  selected,
  select,
  openSettings,
}: {
  selected: MiniApp;
  select: (app: MiniApp) => void;
  openSettings: (section: SettingsSection) => void;
}) {
  const [height, setHeight] = useState(900);
  const [scrollTop, setScrollTop] = useState(0);
  const contentHeight = Math.max(height, 316);
  return (
    <scroll-view
      className="app-rail-scroll"
      scroll-orientation="vertical"
      bindlayoutchange={(event) => setHeight(event.detail.height)}
      bindscroll={(event) => {
        // web-core 0.26.5 can report -half the content height when offset is zero.
        // Clamp until that upstream fallback is removed; native bounce also stays at zero.
        setScrollTop(Math.max(0, event.detail.scrollTop));
      }}
    >
      <view className="app-rail" style={{ height: `${contentHeight}px` }}>
        <view className="app-selectors">
          {[MiniApp.Lane, MiniApp.Pace, MiniApp.RealQA].map((app, index) => (
            <view
              key={app}
              className={`button app-switch ${selected === app ? "selected" : ""}`}
              accessibility-element
              accessibility-traits="button"
              accessibility-label={app}
              accessibility-value={
                selected === app ? "Selected" : "Not selected"
              }
              focusable
              bindtap={() => select(app)}
              bindkeydown={(e) => {
                if (e.key === "Enter" || e.key === " ") select(app);
              }}
            >
              {selected === app && <view className="app-selection" />}
              <AppIcon app={app} />
              <view
                className="app-tooltip"
                style={{ top: `${28 + index * 56 - scrollTop}px` }}
                accessibility-elements-hidden
              >
                <text>{app}</text>
              </view>
            </view>
          ))}
        </view>
        <view className="app-rail-spacer" />
        <view className="app-utilities">
          {[SettingsSection.Account, SettingsSection.Notifications].map(
            (section, index) => {
              const label =
                section === SettingsSection.Account
                  ? "Account & sync"
                  : "App settings";
              return (
                <view
                  key={section}
                  className="button app-utility"
                  accessibility-element
                  accessibility-traits="button"
                  accessibility-label={label}
                  focusable
                  bindtap={() => openSettings(section)}
                  bindkeydown={(event) => {
                    if (event.key === "Enter" || event.key === " ")
                      openSettings(section);
                  }}
                >
                  <view className="app-icon" accessibility-elements-hidden>
                    <svg
                      style={{ width: "24px", height: "24px" }}
                      content={`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#68758b" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">${utilityIcons[section]}</svg>`}
                    />
                  </view>
                  <view
                    className="app-tooltip"
                    style={{
                      top: `${contentHeight - 120 + index * 56 - scrollTop}px`,
                    }}
                    accessibility-elements-hidden
                  >
                    <text>{label}</text>
                  </view>
                </view>
              );
            },
          )}
        </view>
      </view>
    </scroll-view>
  );
}

export function MiniAppNavigation({ app }: { app: MiniApp }) {
  return (
    <view className="navigation miniapp-navigation">
      <view className="miniapp-brand">
        <AppIcon app={app} />
        <text className="brand">{app}</text>
      </view>
      <view className="nav-items">
        <Button primary>Overview</Button>
      </view>
      <view className="nav-spacer" />
      <text className="muted miniapp-status">
        {app === MiniApp.RealQA
          ? "Saved captures appear in Lane"
          : "Coming soon"}
      </text>
    </view>
  );
}

export function ComingSoon({ app, back }: { app: MiniApp; back: () => void }) {
  return (
    <scroll-view className="page-scroll" scroll-orientation="vertical">
      <view className="coming-soon">
        <view className="coming-soon-icon">
          <AppIcon app={app} />
        </view>
        <text className="label">{app}</text>
        <text className="coming-soon-title" accessibility-heading>
          Coming soon
        </text>
        <text className="muted">{app} is not available yet.</text>
        <Button primary onClick={back}>
          Back to Lane
        </Button>
      </view>
    </scroll-view>
  );
}
