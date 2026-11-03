// SPDX-License-Identifier: Apache-2.0
import { base64Decode, base64Encode } from "@bufbuild/protobuf/wire";
import { HostPlatform, type HostInfo, type HostModule } from "../../src/host";
import { MockApi, type MockOptions } from "./mock-api";

type Listener = (value: unknown) => void;
export class FakeEmitter {
  listeners = new Map<string, Set<Listener>>();
  addListener(name: string, listener: Listener) {
    if (!this.listeners.has(name)) this.listeners.set(name, new Set());
    this.listeners.get(name)!.add(listener);
  }
  removeListener(name: string, listener: Listener) {
    this.listeners.get(name)?.delete(listener);
  }
  emit(name: string, value: unknown) {
    for (const listener of this.listeners.get(name) ?? []) listener(value);
  }
}

export interface FakeHostOptions extends MockOptions {
  platform?: HostPlatform;
  signedIn?: boolean;
  /** Persisted widget cache from a previous launch. */
  cache?: Map<string, string>;
}

/** AnyDashHost backed by the in-memory API, cache and clipboard. */
export class FakeHost implements HostModule {
  readonly api: MockApi;
  readonly cache: Map<string, string>;
  readonly calls: { method: string; payload: Record<string, unknown> }[] = [];
  readonly copied: string[] = [];
  readonly opened: string[] = [];
  readonly routes: string[] = [];
  mobileSnapshots: string[] = [];
  signedIn: boolean;
  readonly info: HostInfo;

  constructor(readonly options: FakeHostOptions = {}) {
    this.api = new MockApi(options);
    this.cache = options.cache ?? new Map();
    this.signedIn = options.signedIn ?? true;
    const platform = options.platform ?? HostPlatform.Web;
    this.info = {
      platform,
      collector: platform === HostPlatform.Desktop,
      platformLabel: platform === HostPlatform.Desktop ? "macOS arm64" : "Web",
      hostVersion: "0.1.0-test",
    };
  }

  private async answer(method: string, payload: Record<string, unknown>) {
    switch (method) {
      case "info":
        return this.info;
      case "session":
        return this.signedIn ? { signedIn: true, userKey: "user-key" } : { signedIn: false };
      case "signIn":
        this.signedIn = true;
        return {};
      case "signOut":
        this.signedIn = false;
        return {};
      case "rpc": {
        if (payload.target === "collector" && this.info.platform !== HostPlatform.Desktop)
          throw new Error("The local collector is unavailable.");
        const reply = await this.api.handle(
          String(payload.service),
          String(payload.method),
          base64Decode(String(payload.body ?? "")),
        );
        return { status: reply.status, body: base64Encode(reply.body), correlationId: reply.correlationId };
      }
      case "copyText":
        this.copied.push(String(payload.text));
        return {};
      case "openURL":
        this.opened.push(String(payload.url));
        return {};
      case "cacheRead":
        return { value: this.cache.get(String(payload.key)) };
      case "cacheWrite":
        this.cache.set(String(payload.key), String(payload.value));
        return {};
      case "cacheRemove":
        for (const key of [...this.cache.keys()])
          if (key.startsWith(String(payload.prefix))) this.cache.delete(key);
        return {};
      case "widgetSnapshots":
        if (payload.replaceAll) this.mobileSnapshots = [];
        this.mobileSnapshots.push(...((payload.snapshots as string[]) ?? []));
        return {};
      case "route":
        this.routes.push(String(payload.path));
        return {};
      case "autostart":
        return { enabled: payload.enabled === true };
      case "update":
        return { phase: "unavailable", installedVersion: "0.1.0", message: "Updates are unavailable in tests." };
      default:
        throw new Error(`Unsupported host action ${method}.`);
    }
  }

  call(method: string, raw: string, callback: (raw: unknown) => void) {
    const payload = raw ? (JSON.parse(raw) as Record<string, unknown>) : {};
    this.calls.push({ method, payload });
    this.answer(method, payload).then(
      (value) => callback(JSON.stringify({ ok: true, value })),
      (error: Error) => callback(JSON.stringify({ ok: false, error: { message: error.message } })),
    );
  }
}
