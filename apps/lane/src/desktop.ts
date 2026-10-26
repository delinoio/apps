// SPDX-License-Identifier: Apache-2.0
import type { LaneStoreModule } from "./adapter";

export interface DesktopBridge {
  call(
    channel: string,
    request: {
      module: string;
      method: string;
      args: string[];
    },
    callback: (raw: unknown) => void,
  ): void;
}
export interface HostModule {
  update?(value: string, callback: (raw: string) => void): void;
  realqa?(value: string, callback: (raw: string) => void): void;
  copyText(value: string, callback: (raw: string) => void): void;
  openURL(value: string, callback: (raw: string) => void): void;
  cancelReauthentication(value: string, callback: (raw: string) => void): void;
}
export function desktopModules(bridge: DesktopBridge): {
  store: LaneStoreModule;
  host: HostModule;
} {
  const call = (
    module: string,
    method: string,
    args: string[],
    callback: (raw: string) => void,
  ) => {
    const unavailable = () =>
      callback(
        JSON.stringify({
          ok: false,
          error: {
            reason: "ERROR_REASON_DEPENDENCY_UNAVAILABLE",
            message: "Native service is unavailable. Local data is retained.",
            retryable: true,
          },
        }),
      );
    try {
      // Lynxtron 0.0.30 requires the third callback argument and returns void.
      // Keep this form while pinned to that runtime, even though newer docs
      // show a Promise-returning bridge.call API.
      bridge.call("lane.native", { module, method, args }, (raw) => {
        if (typeof raw === "string") callback(raw);
        else unavailable();
      });
    } catch {
      unavailable();
    }
  };
  return {
    store: {
      open: (request, callback) =>
        call("LaneStore", "open", [request], callback),
      invoke: (method, request, callback) =>
        call("LaneStore", "invoke", [method, request], callback),
      close: (space, callback) => call("LaneStore", "close", [space], callback),
    },
    host: {
      update: (value, callback) =>
        call("LaneHost", "update", [value], callback),
      realqa: (value, callback) =>
        call("LaneHost", "realqa", [value], callback),
      copyText: (value, callback) =>
        call("LaneHost", "copyText", [value], callback),
      openURL: (value, callback) =>
        call("LaneHost", "openURL", [value], callback),
      cancelReauthentication: (value, callback) =>
        call("LaneHost", "cancelReauthentication", [value], callback),
    },
  };
}
