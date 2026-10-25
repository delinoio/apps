// SPDX-License-Identifier: Apache-2.0
import type { LaneAdapter, LocalStatus } from "./adapter.ts";

export interface ReconnectTarget {
  spaceId: string;
  accountId: string;
}
export function reconnectTarget(status: LocalStatus): ReconnectTarget {
  if (!status.reconnectRequired || !status.accountId?.trim() || !status.spaceId)
    throw new Error(
      "Refresh settings to confirm the account before reconnecting.",
    );
  return { spaceId: status.spaceId, accountId: status.accountId };
}
// Called only by the affirmative dialog action. Refreshing status never initiates approval.
export async function confirmReconnect(
  adapter: LaneAdapter,
  target: ReconnectTarget,
) {
  const current = reconnectTarget(await adapter.local("GetLocalStatus", {}));
  if (
    current.spaceId !== target.spaceId ||
    current.accountId !== target.accountId
  )
    throw new Error(
      "The account or data space changed. Start a new reconnection.",
    );
  return adapter.local("ConnectSync", { accountId: target.accountId });
}
