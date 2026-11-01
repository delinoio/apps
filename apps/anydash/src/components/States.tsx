// SPDX-License-Identifier: Apache-2.0
import type { ReactNode } from "@lynx-js/react";
import { Button, Variant } from "@packages/lynx-ui";
import { Copy } from "../copy";
import { quotaDefaultLimit, quotaThings, type Problem } from "../errors";
import { LayoutMode } from "../model/grid";
import { Heading, Notice, NoticeTone, Skeleton, T } from "./primitives";

function Center({ children }: { children: ReactNode }) {
  return (
    <view className="ad-screen">
      <view className="ad-center">{children}</view>
    </view>
  );
}

export function ConnectDeviceState({
  onPrimary,
  primaryLabel = Copy.downloadApp,
  onInstallCli,
}: {
  onPrimary: () => void;
  primaryLabel?: string;
  onInstallCli: () => void;
}) {
  return (
    <Center>
      <Heading>{Copy.connectDevice}</Heading>
      <T muted>{Copy.connectDeviceDetail}</T>
      <view className="ad-actions">
        <Button onPress={onPrimary}>{primaryLabel}</Button>
        <Button variant={Variant.Outline} onPress={onInstallCli}>
          {Copy.installCli}
        </Button>
      </view>
      <T className="ad-small">{Copy.teamWithoutDevice}</T>
    </Center>
  );
}

export function NoDashboardsState({
  onCreate,
}: {
  /** Absent when the space does not grant CREATE_DASHBOARDS. */
  onCreate?: () => void;
}) {
  return (
    <Center>
      <Heading>{Copy.noDashboards}</Heading>
      <T muted>{Copy.noDashboardsDetail}</T>
      {onCreate ? (
        <Button onPress={onCreate}>{Copy.createDashboard}</Button>
      ) : null}
      <T className="ad-small">{Copy.dashboardLimit}</T>
    </Center>
  );
}

const across = (mode: LayoutMode) =>
  mode === LayoutMode.Wide ? 3 : mode === LayoutMode.Tablet ? 2 : 1;
export function LoadingState({ mode }: { mode: LayoutMode }) {
  const count = across(mode);
  return (
    <view
      className="ad-screen"
      accessibility-element
      accessibility-label={Copy.loadingDashboard}
      accessibility-role-description="status"
    >
      <view className="lui-stack lui-column lui-gap-8">
        <Skeleton width="40%" height={18} />
        <view className="lui-stack lui-row lui-gap-8">
          {Array.from({ length: count }, (_, index) => (
            <view key={index} style={{ flex: 1 }}>
              <Skeleton width="100%" height={80} />
            </view>
          ))}
        </view>
        <view className="ad-center" style={{ padding: "0px" }}>
          <T className="ad-small">{Copy.loadingDashboard}</T>
        </view>
      </view>
    </view>
  );
}

export function AccessDeniedState({ onBack }: { onBack: () => void }) {
  return (
    <Center>
      <Heading>{Copy.accessDenied}</Heading>
      <T muted>{Copy.accessDeniedDetail}</T>
      <Button variant={Variant.Outline} onPress={onBack}>
        {Copy.goToDashboards}
      </Button>
    </Center>
  );
}

export interface SupportIds {
  correlationId?: string;
  deviceId?: string;
  connectionId?: string;
  collectorVersion?: string;
}
export const supportDetails = (ids: SupportIds) =>
  [
    ids.correlationId && `${Copy.correlationId} ${ids.correlationId}`,
    ids.deviceId && `${Copy.deviceId} ${ids.deviceId}`,
    ids.connectionId && `${Copy.connectionId} ${ids.connectionId}`,
    ids.collectorVersion && `${Copy.collector} ${ids.collectorVersion}`,
  ].filter((line): line is string => !!line);

export function ErrorState({
  ids,
  onRetry,
  onCopy,
  title = Copy.somethingWrong,
  detail = Copy.somethingWrongDetail,
}: {
  ids: SupportIds;
  onRetry: () => void;
  onCopy: (text: string) => void;
  title?: string;
  detail?: string;
}) {
  const lines = supportDetails(ids);
  return (
    <Center>
      <Heading>{title}</Heading>
      <T muted>{detail}</T>
      {lines.length ? (
        <view className="ad-ids">
          {lines.map((line) => (
            <text key={line} className="ad-mono">
              {line}
            </text>
          ))}
        </view>
      ) : null}
      <view className="ad-actions">
        <Button onPress={onRetry}>{Copy.tryAgain}</Button>
        <Button
          variant={Variant.Outline}
          onPress={() => onCopy(lines.join("\n"))}
        >
          {Copy.copyDetails}
        </Button>
      </view>
    </Center>
  );
}

export function RecoveryState({ onContinue }: { onContinue: () => void }) {
  return (
    <Center>
      <Heading>{Copy.dashboardChanged}</Heading>
      <T muted>{Copy.dashboardChangedDetail}</T>
      <Button onPress={onContinue}>{Copy.continueEditing}</Button>
    </Center>
  );
}

export function UpdateRequiredBanner() {
  return (
    <Notice tone={NoticeTone.Warning} title={`${Copy.updateRequired}.`}>
      {Copy.updateRequiredDetail}
    </Notice>
  );
}

export function LimitReached({ problem }: { problem: Problem }) {
  return (
    <Notice tone={NoticeTone.Danger} title={Copy.limitReached}>
      {Copy.limitReachedDetail(
        problem.quotaLimit || quotaDefaultLimit(problem.quotaKind),
        quotaThings(problem.quotaKind),
      )}
    </Notice>
  );
}

export function SpaceUnavailableState() {
  return (
    <Center>
      <Heading>{Copy.spaceUnavailable}</Heading>
      <T muted>{Copy.spaceUnavailableDetail}</T>
    </Center>
  );
}

export function DuplicateNotice({ devices }: { devices: number }) {
  return (
    <Notice tone={NoticeTone.Info} title={Copy.duplicateDevices(devices)}>
      {Copy.duplicateDevicesDetail}
    </Notice>
  );
}
