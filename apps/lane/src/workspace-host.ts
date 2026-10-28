// SPDX-License-Identifier: Apache-2.0
import { desktopModules, type DesktopBridge, type HostModule } from "./desktop";

export function reportWorkspaceRoute(path: string, requestId?: number) {
  "background only";
  // Browser history belongs to the web host; native hosts keep their own links.
  if (NativeModules.LaneOnline)
    (NativeModules.LaneHost as HostModule | undefined)?.workspace?.(
      JSON.stringify({ route: { path, requestId } }),
      () => {},
    );
}

export function reportWorkspaceState(
  dirty: boolean,
  busy: boolean,
  exit = false,
) {
  "background only";
  const host = (NativeModules.LaneHost ||
    (NativeModules.bridge &&
      desktopModules(NativeModules.bridge as unknown as DesktopBridge)
        .host)) as HostModule | undefined;
  // Mobile hosts can omit the optional window-lifecycle adapter. Tab state stays
  // memory-only; OS process termination never implies draft recovery.
  host?.workspace?.(JSON.stringify({ dirty, busy, exit }), () => {});
}
export function listenWorkspaceExit(requested: () => void) {
  "background only";
  const events = lynx.getJSModule("GlobalEventEmitter");
  events.addListener("lane.workspace.exitRequested", requested);
  return () => events.removeListener("lane.workspace.exitRequested", requested);
}
