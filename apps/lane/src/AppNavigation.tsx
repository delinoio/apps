// SPDX-License-Identifier: Apache-2.0
import { Button } from "./components";
import handrailIcon from "../assets/handrail-icon.png?inline";
import paceIcon from "../assets/pace-icon.png?inline";

export enum MiniApp {
  Lane = "Lane",
  Pace = "Pace",
  Handrail = "Handrail",
}

// Keep marks compatible with native hosts without requiring an SVG renderer.
export function AppIcon({ app }: { app: MiniApp }) {
  return (
    <view
      className={`app-icon icon-${app.toLowerCase()}`}
      accessibility-elements-hidden
    >
      {app === MiniApp.Lane ? (
        <text className="lane-ribbon">▰</text>
      ) : app === MiniApp.Pace ? (
        <image className="pace-artwork" src={paceIcon} mode="aspectFit" />
      ) : (
        <image
          className="handrail-artwork"
          src={handrailIcon}
          mode="aspectFit"
        />
      )}
    </view>
  );
}

export function AppRail({
  selected,
  select,
}: {
  selected: MiniApp;
  select: (app: MiniApp) => void;
}) {
  return (
    <view className="app-rail">
      {[MiniApp.Lane, MiniApp.Pace, MiniApp.Handrail].map((app) => (
        <view
          key={app}
          className={`button app-switch ${selected === app ? "selected" : ""}`}
          accessibility-element
          accessibility-traits="button"
          accessibility-label={app}
          accessibility-value={selected === app ? "Selected" : "Not selected"}
          focusable
          bindtap={() => select(app)}
          bindkeydown={(e) => {
            if (e.key === "Enter" || e.key === " ") select(app);
          }}
        >
          {selected === app && <view className="app-selection" />}
          <AppIcon app={app} />
          <view className="app-tooltip" accessibility-elements-hidden>
            <text>{app}</text>
          </view>
        </view>
      ))}
    </view>
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
      <text className="muted miniapp-status">Coming soon</text>
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
