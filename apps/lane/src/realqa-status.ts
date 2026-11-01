// SPDX-License-Identifier: Apache-2.0
// Maps desktop host status identifiers to the English text shown in RealQA.
import type { RealQAState } from "./realqa-host";

// Values sent by the macOS (`granted`, `not_granted`) and Windows (`available`)
// capture hosts in `RealQAState.permission`.
export enum CapturePermission {
  Granted = "granted",
  NotGranted = "not_granted",
  Available = "available",
}
// Values sent by the desktop host in `RealQAState.chrome.state`.
export enum ChromeConnectionState {
  Disconnected = "disconnected",
  ApprovalPending = "approval_pending",
  Connected = "connected",
  Error = "error",
}

const permissionLabels: Record<CapturePermission, string> = {
  [CapturePermission.Granted]: "Granted",
  [CapturePermission.NotGranted]: "Not granted",
  [CapturePermission.Available]: "Available",
};
const chromeLabels: Record<ChromeConnectionState, string> = {
  [ChromeConnectionState.Disconnected]: "Disconnected",
  [ChromeConnectionState.ApprovalPending]: "Approval pending",
  [ChromeConnectionState.Connected]: "Connected",
  [ChromeConnectionState.Error]: "Error",
};

const lookup = (labels: Record<string, string>, value?: string) =>
  value !== undefined && Object.prototype.hasOwnProperty.call(labels, value)
    ? labels[value]
    : undefined;

export function permissionLabel(state?: RealQAState): string {
  if (!state) return "Loading…";
  return lookup(permissionLabels, state.permission) ?? "Unavailable";
}

// Hosts without a Chrome bridge omit `chrome`; that is a disconnected state.
export function chromeConnectionLabel(state?: RealQAState): string {
  if (!state) return "Loading…";
  if (!state.chrome) return chromeLabels[ChromeConnectionState.Disconnected];
  return lookup(chromeLabels, state.chrome.state) ?? "Status unavailable";
}

// `Install extension` opens only a published HTTPS listing.
export function chromeListingUrl(state?: RealQAState): string | undefined {
  const url = state?.chrome?.listingUrl?.trim();
  return url && /^https:\/\/[^\s/?#]+/i.test(url) ? url : undefined;
}
