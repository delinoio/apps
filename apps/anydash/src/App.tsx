// SPDX-License-Identifier: Apache-2.0
import { useState } from "@lynx-js/react";
import { Button, Theme, UIProvider } from "@packages/lynx-ui";
import { Heading, T } from "./components/primitives";
import { ErrorState, UpdateRequiredBanner } from "./components/States";
import { Copy, ExtraCopy } from "./copy";
import { hostCall, HostMethod, type HostInfo } from "./host";
import { logError } from "./log";
import { layoutMode, LayoutMode } from "./model/grid";
import { RouteKind } from "./model/routes";
import { NavigationProvider, useNavigation } from "./navigation";
import { DeviceApproval } from "./screens/DeviceApproval";
import { Home } from "./screens/Home";
import { Settings } from "./screens/Settings";
import { SessionPhase, SessionProvider, useSession } from "./session";

function SignIn() {
  const session = useSession();
  const busy = session.phase === SessionPhase.SigningIn;
  return (
    <view
      className="ad-screen"
      style={{ maxWidth: "460px", alignSelf: "center", width: "100%" }}
    >
      <view className="ad-center">
        <text className="ad-logo">{Copy.appName}</text>
        <Heading>{ExtraCopy.signInTitle}</Heading>
        <T muted>{ExtraCopy.signInDetail}</T>
        <Button
          onPress={() => void session.signIn()}
          loading={busy}
          loadingLabel={ExtraCopy.signingIn}
        >
          {ExtraCopy.signIn}
        </Button>
      </view>
    </view>
  );
}

function Routes({ mode }: { mode: LayoutMode }) {
  const session = useSession();
  const { route, navigate } = useNavigation();
  if (
    session.phase === SessionPhase.Starting ||
    session.phase === SessionPhase.Connecting
  )
    return (
      <view className="ad-screen">
        <view className="ad-center">
          <T className="ad-small">{ExtraCopy.connecting}</T>
        </view>
      </view>
    );
  if (
    session.phase === SessionPhase.SignedOut ||
    session.phase === SessionPhase.SigningIn
  )
    return <SignIn />;
  if (session.phase === SessionPhase.Failed && !session.updateRequired)
    return (
      <ErrorState
        ids={{ correlationId: session.problem?.correlationId }}
        onRetry={session.retry}
        onCopy={(text) => void hostCall(HostMethod.CopyText, { text })}
      />
    );
  if (session.phase === SessionPhase.Failed) return null;
  switch (route.kind) {
    case RouteKind.Connect:
      return (
        <DeviceApproval
          enrollmentId={route.enrollmentId}
          onDone={() => navigate({ kind: RouteKind.Home })}
        />
      );
    case RouteKind.Settings:
      return <Settings />;
    case RouteKind.NotFound:
      return (
        <view className="ad-screen">
          <view className="ad-center">
            <Heading>{ExtraCopy.notFound}</Heading>
            <T muted>{ExtraCopy.notFoundDetail}</T>
            <Button
              onPress={() =>
                navigate({ kind: RouteKind.Home }, { replace: true })
              }
            >
              {Copy.goToDashboards}
            </Button>
          </view>
        </view>
      );
    default:
      return <Home route={route} mode={mode} />;
  }
}

function Frame({ mode }: { mode: LayoutMode }) {
  const session = useSession();
  return (
    <view className="ad-page">
      {session.updateRequired ? <UpdateRequiredBanner /> : null}
      <Routes mode={mode} />
    </view>
  );
}

export function App({
  info,
  initialPath = "/",
  initialWidth,
}: {
  info?: HostInfo;
  initialPath?: string;
  initialWidth?: number;
}) {
  const [width, setWidth] = useState(
    initialWidth ??
      (typeof SystemInfo !== "undefined" && SystemInfo.pixelWidth
        ? SystemInfo.pixelWidth / (SystemInfo.pixelRatio || 1)
        : 1280),
  );
  const theme =
    (lynx.__globalProps as { theme?: string } | undefined)?.theme === "dark"
      ? Theme.Dark
      : Theme.Light;
  return (
    <UIProvider
      theme={theme}
      // A bounded root lets the page scroll view take the overflow.
      style={{ height: "100%" }}
      onHostError={(error) =>
        logError("anydash_host_method_failed", { ...error })
      }
    >
      <view
        className="ad-app"
        bindlayoutchange={(event: { detail?: { width?: number } }) => {
          const next = event.detail?.width;
          if (
            typeof next === "number" &&
            next > 0 &&
            Math.abs(next - width) > 1
          )
            setWidth(next);
        }}
      >
        <SessionProvider info={info}>
          <NavigationProvider initial={initialPath}>
            <scroll-view className="ad-scroll" scroll-orientation="vertical">
              <Frame mode={layoutMode(width)} />
            </scroll-view>
          </NavigationProvider>
        </SessionProvider>
      </view>
    </UIProvider>
  );
}
