// SPDX-License-Identifier: Apache-2.0
// Desktop implementation of the AnyDashHost contract (src/host.ts). The
// renderer can only call these methods; it never chooses a file path, an
// executable or a non-AnyDash network destination.
import { createHash } from "node:crypto";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";

const CORRELATION_HEADER = "x-anydash-correlation-id";
const ALLOWED_HEADERS = new Set([
  "x-anydash-client-kind",
  "x-anydash-client-version",
]);
const RPC_NAME = /^[A-Za-z][A-Za-z0-9_.]*$/;
const COLLECTOR_METHODS = new Set([
  "GetCollectorStatus",
  "StartLogin",
  "Logout",
  "AddConnectionCredential",
  "RemoveConnectionCredential",
  "ListSourcePaths",
  "AddSourcePath",
  "DisableSourcePath",
  "EnableSourcePath",
  "RotateKey",
  "RequestLocalSync",
  "PutNativeWidgetSnapshots",
  "ListNativeWidgetSnapshots",
]);

export class HostError extends Error {
  constructor(message, code = "HOST_FAILED") {
    super(message);
    this.code = code;
  }
}

export class DesktopHost {
  constructor({
    config,
    auth,
    collector,
    fetcher = fetch,
    clipboard,
    openExternal,
    loginItems,
    updater,
    cacheDirectory,
    platformLabel,
    onRoute = () => {},
  }) {
    Object.assign(this, {
      config,
      auth,
      collector,
      fetcher,
      clipboard,
      openExternal,
      loginItems,
      updater,
      cacheDirectory,
      platformLabel,
      onRoute,
    });
  }

  // Cache files are named by a hash so renderer keys never become paths.
  #cacheFile(key) {
    return join(
      this.cacheDirectory,
      `${createHash("sha256").update(String(key)).digest("hex")}.bin`,
    );
  }
  #indexFile() {
    return join(this.cacheDirectory, "index.json");
  }
  async #index() {
    try {
      return JSON.parse(await readFile(this.#indexFile(), "utf8"));
    } catch {
      return {};
    }
  }

  async call(method, payload) {
    switch (method) {
      case "info":
        return {
          platform: "desktop",
          collector: true,
          platformLabel: this.platformLabel,
          hostVersion: this.config.version,
        };
      case "session":
        return (await this.auth.signedIn())
          ? { signedIn: true, userKey: await this.auth.userKey() }
          : { signedIn: false };
      case "signIn":
        await this.auth.signIn();
        return {};
      case "signOut":
        await this.auth.signOut();
        await rm(this.cacheDirectory, { recursive: true, force: true });
        return {};
      case "delcoreToken":
        return {
          token: await this.auth.accessToken(this.config.delcoreResource),
        };
      case "rpc":
        return payload.target === "collector"
          ? this.#collector(payload)
          : this.#api(payload);
      case "copyText":
        if (typeof payload.text !== "string")
          throw new HostError("Nothing to copy.");
        this.clipboard.writeText(payload.text);
        return {};
      case "openURL": {
        const url = new URL(String(payload.url));
        if (url.protocol !== "https:")
          throw new HostError("Only HTTPS links can be opened.");
        await this.openExternal(url.href);
        return {};
      }
      case "cacheRead": {
        try {
          return {
            value: (await readFile(this.#cacheFile(payload.key))).toString(
              "utf8",
            ),
          };
        } catch {
          return {};
        }
      }
      case "cacheWrite": {
        await mkdir(this.cacheDirectory, { recursive: true });
        await writeFile(
          this.#cacheFile(payload.key),
          String(payload.value ?? ""),
        );
        const index = await this.#index();
        index[String(payload.key)] = true;
        await writeFile(this.#indexFile(), JSON.stringify(index));
        return {};
      }
      case "cacheRemove": {
        const prefix = String(payload.prefix ?? "");
        const index = await this.#index();
        for (const key of Object.keys(index))
          if (key.startsWith(prefix)) {
            await rm(this.#cacheFile(key), { force: true });
            delete index[key];
          }
        await mkdir(this.cacheDirectory, { recursive: true });
        await writeFile(this.#indexFile(), JSON.stringify(index));
        return {};
      }
      case "widgetSnapshots":
        // Desktop widget hosts read snapshots from the collector instead.
        return {};
      case "autostart": {
        if (typeof payload.enabled === "boolean")
          this.loginItems.set(payload.enabled);
        return { enabled: this.loginItems.get() };
      }
      case "update":
        return this.updater.act(String(payload.action ?? "status"));
      case "route":
        this.onRoute(String(payload.path ?? "/"));
        return {};
      default:
        throw new HostError("Unsupported host action.", "UNSUPPORTED");
    }
  }

  async #collector(payload) {
    if (
      payload.service !== "anydash.v1.LocalCollectorService" ||
      !COLLECTOR_METHODS.has(payload.method)
    )
      throw new HostError("Unsupported collector request.");
    const reply = await this.collector.forward(
      payload.method,
      Buffer.from(String(payload.body ?? ""), "base64"),
    );
    return {
      status: reply.status,
      body: reply.body.toString("base64"),
      contentType: reply.contentType,
    };
  }

  async #api(payload) {
    const service = String(payload.service ?? "");
    const method = String(payload.method ?? "");
    if (
      !RPC_NAME.test(service) ||
      !RPC_NAME.test(method) ||
      !service.startsWith("anydash.v1.") ||
      service === "anydash.v1.LocalCollectorService"
    )
      throw new HostError("Invalid RPC target.");
    const headers = {
      "content-type": "application/proto",
      "connect-protocol-version": "1",
    };
    for (const [key, value] of Object.entries(payload.headers ?? {}))
      if (ALLOWED_HEADERS.has(key.toLowerCase()) && typeof value === "string")
        headers[key] = value;
    if (await this.auth.signedIn())
      headers.authorization = `Bearer ${await this.auth.accessToken(this.config.anydashResource)}`;
    const response = await this.fetcher(
      `${this.config.api}/${service}/${method}`,
      {
        method: "POST",
        headers,
        body: Buffer.from(String(payload.body ?? ""), "base64"),
      },
    );
    return {
      status: response.status,
      body: Buffer.from(await response.arrayBuffer()).toString("base64"),
      contentType: response.headers.get("content-type") ?? undefined,
      correlationId: response.headers.get(CORRELATION_HEADER) ?? undefined,
    };
  }

  /** Reply envelope consumed by src/host.ts hostCall(). */
  async reply(method, raw) {
    try {
      const payload = raw ? JSON.parse(raw) : {};
      return JSON.stringify({
        ok: true,
        value: await this.call(method, payload),
      });
    } catch (error) {
      return JSON.stringify({
        ok: false,
        error: {
          message:
            error instanceof Error ? error.message : "The desktop host failed.",
          code: error?.code ?? "HOST_FAILED",
        },
      });
    }
  }
}
