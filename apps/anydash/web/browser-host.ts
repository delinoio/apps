// SPDX-License-Identifier: Apache-2.0
// Browser implementation of the AnyDashHost contract (src/host.ts). It runs on
// the page, outside the Lynx worker: it owns Logto PKCE sign-in, adds the
// AnyDash bearer token to API requests, injects the Delcore token into the
// requests that carry it and keeps tokens out of the bundle.

export interface BrowserConfig {
  api: string;
  endpoint: string;
  appId: string;
  anydashResource: string;
  delcoreResource: string;
  version: string;
}

export interface AuthClient {
  isAuthenticated(): Promise<boolean>;
  signIn(redirectUri: string): Promise<void>;
  signOut(postLogoutRedirectUri?: string): Promise<void>;
  getAccessToken(resource?: string): Promise<string>;
  getIdTokenClaims(): Promise<{ sub: string }>;
}

export interface BrowserDeps {
  config: BrowserConfig;
  auth?: AuthClient;
  fetch: typeof fetch;
  location: { origin: string; pathname: string; search: string };
  history: {
    pushState(data: unknown, unused: string, url: string): void;
    replaceState(data: unknown, unused: string, url: string): void;
  };
  clipboard?: { writeText(text: string): Promise<void> };
  open(url: string): void;
  digest(text: string): Promise<string>;
  savePath(path: string): void;
}

export enum CallMethod {
  Info = "info",
  Session = "session",
  SignIn = "signIn",
  SignOut = "signOut",
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

export const CALLBACK_PATH = "/callback";
const CORRELATION_HEADER = "x-anydash-correlation-id";
const ALLOWED_HEADERS = new Set([
  "x-anydash-client-kind",
  "x-anydash-client-version",
]);
const RPC_NAME = /^[A-Za-z][A-Za-z0-9_.]*$/;
/**
 * Requests whose field 1 is `string delcore_access_token`. The host appends the
 * Delcore resource token so it never crosses into the Lynx bundle.
 */
const DELCORE_TOKEN_METHODS = new Set([
  "anydash.v1.AccountService/ConnectAccount",
  "anydash.v1.DashboardService/ListSpaces",
]);

/**
 * Appends `delcore_access_token` (field 1, length-delimited) to a binary
 * Protobuf request. A later occurrence of a singular field wins when the server
 * parses the message, so a value set by the bundle is replaced.
 */
export function withDelcoreToken(body: Uint8Array, token: string) {
  const value = new TextEncoder().encode(token);
  const length: number[] = [];
  let remaining = value.length;
  while (remaining > 0x7f) {
    length.push((remaining & 0x7f) | 0x80);
    remaining >>>= 7;
  }
  length.push(remaining);
  const result = new Uint8Array(body.length + 1 + length.length + value.length);
  result.set(body);
  result[body.length] = 0x0a;
  result.set(length, body.length + 1);
  result.set(value, body.length + 1 + length.length);
  return result;
}

export class HostCallError extends Error {
  constructor(
    message: string,
    readonly code = "HOST_FAILED",
  ) {
    super(message);
  }
}

function base64ToBytes(value: string) {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++)
    bytes[index] = binary.charCodeAt(index);
  return bytes;
}
function bytesToBase64(bytes: Uint8Array) {
  let binary = "";
  for (let index = 0; index < bytes.length; index += 0x8000)
    binary += String.fromCharCode(...bytes.subarray(index, index + 0x8000));
  return btoa(binary);
}

export class BrowserHost {
  constructor(private readonly deps: BrowserDeps) {}

  private requireAuth() {
    if (!this.deps.auth)
      throw new HostCallError(
        "Sign-in is not configured for this build.",
        "AUTH_UNCONFIGURED",
      );
    return this.deps.auth;
  }

  async call(
    method: string,
    payload: Record<string, unknown>,
  ): Promise<unknown> {
    const { deps } = this;
    switch (method) {
      case CallMethod.Info:
        return {
          platform: "web",
          collector: false,
          platformLabel: "Web",
          hostVersion: deps.config.version,
        };
      case CallMethod.Session: {
        const auth = deps.auth;
        if (!auth || !(await auth.isAuthenticated()))
          return { signedIn: false };
        const claims = await auth.getIdTokenClaims();
        return {
          signedIn: true,
          userKey: await deps.digest(`anydash:${claims.sub}`),
        };
      }
      case CallMethod.SignIn: {
        const route = `${deps.location.pathname}${deps.location.search}`;
        if (route !== CALLBACK_PATH) deps.savePath(route);
        // Full-page redirect; the page reloads at /callback afterwards.
        await this.requireAuth().signIn(
          `${deps.location.origin}${CALLBACK_PATH}`,
        );
        return {};
      }
      case CallMethod.SignOut:
        await this.requireAuth().signOut(deps.location.origin);
        return {};
      case CallMethod.Rpc:
        return this.rpc(payload);
      case CallMethod.CopyText:
        if (!deps.clipboard || typeof payload.text !== "string")
          throw new HostCallError("Copying is unavailable in this browser.");
        await deps.clipboard.writeText(payload.text);
        return {};
      case CallMethod.OpenURL: {
        const target = new URL(String(payload.url));
        if (target.protocol !== "https:")
          throw new HostCallError("Only HTTPS links can be opened.");
        deps.open(target.href);
        return {};
      }
      // The web keeps only the in-memory React Query cache; nothing persists.
      case CallMethod.CacheRead:
        return {};
      case CallMethod.CacheWrite:
      case CallMethod.CacheRemove:
      case CallMethod.WidgetSnapshots:
        return {};
      case CallMethod.Route: {
        const path = String(payload.path ?? "/");
        if (!path.startsWith("/") || path.startsWith("//"))
          throw new HostCallError("Invalid route.");
        if (payload.replace) deps.history.replaceState({}, "", path);
        else deps.history.pushState({}, "", path);
        return {};
      }
      case CallMethod.Autostart:
      case CallMethod.Update:
        throw new HostCallError(
          "This action is available in the desktop app.",
          "UNSUPPORTED",
        );
      default:
        throw new HostCallError("Unsupported host action.", "UNSUPPORTED");
    }
  }

  private async rpc(payload: Record<string, unknown>) {
    const { deps } = this;
    if (payload.target !== "api")
      throw new HostCallError(
        "The local collector is available in the desktop app.",
        "UNSUPPORTED",
      );
    const service = String(payload.service ?? "");
    const method = String(payload.method ?? "");
    if (
      !RPC_NAME.test(service) ||
      !RPC_NAME.test(method) ||
      !service.startsWith("anydash.v1.")
    )
      throw new HostCallError("Invalid RPC target.");
    const headers: Record<string, string> = {
      "Content-Type": "application/proto",
      "Connect-Protocol-Version": "1",
    };
    const requested = (payload.headers ?? {}) as Record<string, unknown>;
    for (const [key, value] of Object.entries(requested))
      if (ALLOWED_HEADERS.has(key.toLowerCase()) && typeof value === "string")
        headers[key] = value;
    const auth = deps.auth;
    let body = base64ToBytes(String(payload.body ?? ""));
    if (auth && (await auth.isAuthenticated())) {
      headers.Authorization = `Bearer ${await auth.getAccessToken(deps.config.anydashResource)}`;
      if (DELCORE_TOKEN_METHODS.has(`${service}/${method}`))
        body = withDelcoreToken(
          body,
          await auth.getAccessToken(deps.config.delcoreResource),
        );
    }
    const response = await deps.fetch(
      `${deps.config.api}/${service}/${method}`,
      { method: "POST", headers, body, credentials: "omit" },
    );
    return {
      status: response.status,
      body: bytesToBase64(new Uint8Array(await response.arrayBuffer())),
      contentType: response.headers.get("content-type") ?? undefined,
      correlationId: response.headers.get(CORRELATION_HEADER) ?? undefined,
    };
  }

  /** Reply envelope consumed by src/host.ts hostCall(). */
  async reply(method: string, raw: string) {
    try {
      const payload = raw ? (JSON.parse(raw) as Record<string, unknown>) : {};
      return JSON.stringify({
        ok: true,
        value: await this.call(method, payload),
      });
    } catch (error) {
      return JSON.stringify({
        ok: false,
        error: {
          message:
            error instanceof Error ? error.message : "The browser host failed.",
          code: error instanceof HostCallError ? error.code : "HOST_FAILED",
        },
      });
    }
  }
}
