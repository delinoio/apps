// SPDX-License-Identifier: Apache-2.0
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

export function NewTabPage({ open }: { open: (app: MiniApp) => void }) {
  const descriptions = {
    [MiniApp.Lane]: "Tasks and repositories",
    [MiniApp.Pace]: "Coming soon",
    [MiniApp.RealQA]: "Capture and review",
  };
  return (
    <scroll-view className="page-scroll" scroll-orientation="vertical">
      <view className="new-tab-page">
        <text className="coming-soon-title" accessibility-heading>
          New tab
        </text>
        <text className="muted">Choose a mini-app to open.</text>
        <view className="miniapp-picker">
          {Object.values(MiniApp).map((app) => (
            <view
              key={app}
              className="button miniapp-card"
              accessibility-element
              accessibility-traits="button"
              accessibility-label={app}
              focusable
              bindtap={() => open(app)}
              bindkeydown={(event) => {
                if (event.key === "Enter" || event.key === " ") open(app);
              }}
            >
              <AppIcon app={app} />
              <text className="heading">{app}</text>
              <text className="muted">{descriptions[app]}</text>
            </view>
          ))}
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
