// SPDX-License-Identifier: Apache-2.0
import { timestampDate, type Timestamp } from "@bufbuild/protobuf/wkt";
import type {
  Connection,
  Dashboard,
  Device,
} from "@repo/protos/anydash/v1/anydash_pb";

/** Dashboard data refreshes every minute while the view is visible. */
export const AUTO_REFRESH_MS = 60_000;
/** Manual refresh is limited to once per minute per connection or device. */
export const MANUAL_REFRESH_MS = 60_000;

export interface RefreshTargets {
  connectionIds: string[];
  deviceIds: string[];
}

/** Sources the caller owns; other owners' widgets only re-query their data. */
export function refreshTargets(
  dashboard: Dashboard | undefined,
  devices: Device[],
): RefreshTargets {
  const connections = new Set<string>();
  const deviceIds = new Set<string>();
  for (const widget of dashboard?.widgets ?? []) {
    if (!widget.callerIsDataOwner || !widget.source) continue;
    for (const id of widget.source.connectionIds) connections.add(id);
    for (const id of widget.source.deviceIds) deviceIds.add(id);
    if (widget.source.allDevices)
      for (const device of devices) deviceIds.add(device.id);
  }
  return { connectionIds: [...connections], deviceIds: [...deviceIds] };
}

const millis = (value: Timestamp | undefined) =>
  value ? timestampDate(value).getTime() : 0;

/** Latest server-reported next manual refresh time for the targets. */
export function serverNextAllowed(
  targets: RefreshTargets,
  connections: Connection[],
) {
  let next = 0;
  for (const connection of connections)
    if (targets.connectionIds.includes(connection.id))
      next = Math.max(next, millis(connection.nextManualRefreshAt));
  return next;
}

export function remainingSeconds(nextAllowedAt: number, now: number) {
  return nextAllowedAt > now ? Math.ceil((nextAllowedAt - now) / 1000) : 0;
}

/** Combines successful responses and RATE_LIMITED waits into one deadline. */
export function nextAllowedFrom(
  now: number,
  results: { nextAllowedAt?: Timestamp; retryAfterSeconds?: number }[],
) {
  let next = now + MANUAL_REFRESH_MS;
  for (const result of results) {
    if (result.nextAllowedAt)
      next = Math.max(next, millis(result.nextAllowedAt));
    if (result.retryAfterSeconds)
      next = Math.max(next, now + result.retryAfterSeconds * 1000);
  }
  return next;
}
