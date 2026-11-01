// SPDX-License-Identifier: Apache-2.0
import { ClientKind } from "@repo/protos/anydash/v1/anydash_pb";

declare const __ANYDASH_VERSION__: string;
/** SemVer sent as X-AnyDash-Client-Version; injected by the Rspeedy build. */
export const APP_VERSION: string = __ANYDASH_VERSION__;

export enum HostPlatform {
  Web = "web",
  Desktop = "desktop",
  Ios = "ios",
  Android = "android",
}

export enum HostMethod {
  Info = "info",
  Session = "session",
  SignIn = "signIn",
  SignOut = "signOut",
  DelcoreToken = "delcoreToken",
  Rpc = "rpc",
  CopyText = "copyText",
  OpenURL = "openURL",
  CacheRead = "cacheRead",
  CacheWrite = "cacheWrite",
  CacheRemove = "cacheRemove",
  WidgetSnapshots = "widgetSnapshots",
  Autostart = "autostart",
  Update = "update",
  Route = "route",
}

export enum RpcTarget {
  /** The AnyDash API; the host adds the Logto user token. */
  Api = "api",
  /** LocalCollectorService over local IPC; desktop hosts only. */
  Collector = "collector",
}

export enum HostEvent {
  Route = "anydash.route",
  Session = "anydash.session",
  Resume = "anydash.resume",
  /** `{ visible: boolean }` when the window or app moves to or from the background. */
  Visibility = "anydash.visibility",
}

export interface HostInfo {
  platform: HostPlatform;
  /** The host has a local collector connection (desktop). */
  collector: boolean;
  /** Short platform label for support details, for example "macOS arm64". */
  platformLabel: string;
  /** Host package version; equals APP_VERSION in release builds. */
  hostVersion: string;
}

export interface SessionState {
  signedIn: boolean;
  /** Opaque, stable per-user key for local caches; never a token. */
  userKey?: string;
}

export interface RpcRequest {
  target: RpcTarget;
  service: string;
  method: string;
  /** Base64 binary Protobuf request body. */
  body: string;
  headers: Record<string, string>;
}

export interface RpcReply {
  status: number;
  /** Base64 response body: binary Protobuf, or Connect JSON for errors. */
  body: string;
  contentType?: string;
  correlationId?: string;
}

export enum UpdatePhase {
  Unavailable = "unavailable",
  Idle = "idle",
  Checking = "checking",
  Current = "current",
  Downloading = "downloading",
  Verifying = "verifying",
  Ready = "ready",
  Installing = "installing",
  Error = "error",
}

export interface UpdateStatus {
  phase: UpdatePhase;
  installedVersion: string;
  availableVersion?: string;
  message: string;
  progress?: number;
}

export enum UpdateAction {
  Status = "status",
  Check = "check",
  Install = "install",
}

export interface HostReply<T> {
  ok: boolean;
  value?: T;
  error?: { message?: string; code?: string };
}

export class HostError extends Error {
  constructor(
    message: string,
    readonly code = "HOST_FAILED",
  ) {
    super(message);
    this.name = "HostError";
  }
}

/** The native module contract every AnyDash host implements. */
export interface HostModule {
  call(method: string, payload: string, callback: (raw: unknown) => void): void;
}

interface DesktopBridge {
  call(
    channel: string,
    request: { method: string; payload: string },
    callback: (raw: unknown) => void,
  ): void;
}

let override: HostModule | undefined;
/** Tests and fixtures replace the platform host. */
export function setHostModule(module: HostModule | undefined) {
  override = module;
}

function module(): HostModule | undefined {
  "background only";
  if (override) return override;
  const modules = NativeModules as unknown as Record<string, unknown>;
  if (modules.AnyDashHost) return modules.AnyDashHost as HostModule;
  const bridge = modules.bridge as DesktopBridge | undefined;
  if (bridge)
    return {
      call(method, payload, callback) {
        // Lynxtron 0.0.30 requires the callback argument and returns void.
        // Keep this form while the runtime is pinned to that version.
        bridge.call("anydash.host", { method, payload }, callback);
      },
    };
  return undefined;
}

export function hostCall<T>(method: HostMethod, payload: unknown = {}) {
  "background only";
  return new Promise<T>((resolve, reject) => {
    const host = module();
    if (!host) {
      reject(new HostError("This AnyDash host is unavailable.", "NO_HOST"));
      return;
    }
    try {
      host.call(method, JSON.stringify(payload), (raw) => {
        let reply: HostReply<T>;
        try {
          reply = (
            typeof raw === "string" ? JSON.parse(raw) : raw
          ) as HostReply<T>;
        } catch {
          reject(new HostError("The host returned an unreadable reply."));
          return;
        }
        if (reply?.ok) resolve(reply.value as T);
        else
          reject(
            new HostError(
              reply?.error?.message || "The host operation failed.",
              reply?.error?.code,
            ),
          );
      });
    } catch {
      reject(new HostError("The host operation failed."));
    }
  });
}

export function clientKind(platform: HostPlatform): ClientKind {
  switch (platform) {
    case HostPlatform.Desktop:
      return ClientKind.DESKTOP;
    case HostPlatform.Ios:
      return ClientKind.IOS;
    case HostPlatform.Android:
      return ClientKind.ANDROID;
    default:
      return ClientKind.WEB;
  }
}

export function clientKindHeader(kind: ClientKind): string {
  return `CLIENT_KIND_${ClientKind[kind]}`;
}

type Listener = (value: unknown) => void;
interface Emitter {
  addListener(name: string, listener: Listener): void;
  removeListener(name: string, listener: Listener): void;
}
let emitterOverride: Emitter | undefined;
export function setHostEmitter(emitter: Emitter | undefined) {
  emitterOverride = emitter;
}
export function listenHost(event: HostEvent, listener: Listener) {
  "background only";
  const emitter =
    emitterOverride ??
    (lynx.getJSModule("GlobalEventEmitter") as unknown as Emitter);
  emitter.addListener(event, listener);
  return () => emitter.removeListener(event, listener);
}
