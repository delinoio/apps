// SPDX-License-Identifier: Apache-2.0
import { desktopModules, type DesktopBridge } from "./desktop";

export enum UpdatePhase {
  Idle = "idle",
  Checking = "checking",
  Downloading = "downloading",
  Verifying = "verifying",
  Installing = "installing",
  Current = "current",
  Error = "error",
  Unavailable = "unavailable",
}
export enum UpdateAction {
  Status = "status",
  Start = "start",
  Cancel = "cancel",
}
export interface UpdateStatus {
  phase: UpdatePhase;
  installedRelease: string;
  message: string;
  progress?: number;
}
export function updateAction(
  action: UpdateAction,
): Promise<UpdateStatus | undefined> {
  "background only";
  if (!NativeModules.bridge) return Promise.resolve(undefined);
  const host = desktopModules(
    NativeModules.bridge as unknown as DesktopBridge,
  ).host;
  return new Promise((resolve, reject) => {
    host.update!(action, (raw) => {
      try {
        const reply = JSON.parse(raw);
        if (!reply.ok)
          throw new Error(
            reply.error?.message || "The update service is unavailable.",
          );
        resolve(reply.response as UpdateStatus);
      } catch (error) {
        reject(error);
      }
    });
  });
}
export function updateBusy(phase: UpdatePhase) {
  return [
    UpdatePhase.Checking,
    UpdatePhase.Downloading,
    UpdatePhase.Verifying,
    UpdatePhase.Installing,
  ].includes(phase);
}
