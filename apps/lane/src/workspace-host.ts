// SPDX-License-Identifier: Apache-2.0
import { desktopModules, type DesktopBridge, type HostModule } from "./desktop";
import { tabCommandRequest, type TabCommandRequest } from "./tabs";

export function reportWorkspaceState(
  dirty: boolean,
  busy: boolean,
  exit = false,
  tabs?: {
    count: number;
    history: number;
    selectedIndex: number;
    focusedIndex: number;
  },
) {
  "background only";
  const host = (NativeModules.LaneHost ||
    (NativeModules.bridge &&
      desktopModules(NativeModules.bridge as unknown as DesktopBridge)
        .host)) as HostModule | undefined;
  // Mobile hosts can omit the optional window-lifecycle adapter. Tab state stays
  // memory-only; OS process termination never implies draft recovery.
  host?.workspace?.(JSON.stringify({ dirty, busy, exit, tabs }), () => {});
}
export function listenWorkspaceExit(requested: () => void) {
  "background only";
  const events = lynx.getJSModule("GlobalEventEmitter");
  events.addListener("lane.workspace.exitRequested", requested);
  return () => events.removeListener("lane.workspace.exitRequested", requested);
}
export function listenWorkspaceTabCommands(
  requested: (request: TabCommandRequest) => void,
) {
  "background only";
  const events = lynx.getJSModule("GlobalEventEmitter");
  const onCommand = (raw: unknown) => {
    const request = tabCommandRequest(raw);
    if (request) requested(request);
  };
  events.addListener("lane.workspace.tabCommand", onCommand);
  return () => events.removeListener("lane.workspace.tabCommand", onCommand);
}
