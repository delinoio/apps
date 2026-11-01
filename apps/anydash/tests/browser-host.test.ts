// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "@rstest/core";
import {
  BrowserHost,
  CALLBACK_PATH,
  type BrowserDeps,
} from "../web/browser-host";

function deps(overrides: Partial<BrowserDeps> = {}) {
  const requests: { url: string; init: RequestInit }[] = [];
  const history: string[] = [];
  const opened: string[] = [];
  const saved: string[] = [];
  const value: BrowserDeps = {
    config: {
      api: "https://anydash.api.delino.io",
      endpoint: "https://auth.example",
      appId: "app",
      anydashResource: "https://anydash.api.delino.io",
      delcoreResource: "https://delcore.api.delino.io",
      version: "0.1.0",
    },
    auth: {
      isAuthenticated: async () => true,
      signIn: async () => {},
      signOut: async () => {},
      getAccessToken: async (resource) => `token-for-${resource}`,
      getIdTokenClaims: async () => ({ sub: "user-1" }),
    },
    fetch: (async (url: string, init: RequestInit) => {
      requests.push({ url, init });
      return new Response(new Uint8Array([1, 2, 3]), {
        status: 200,
        headers: {
          "x-anydash-correlation-id": "corr-1",
          "content-type": "application/proto",
        },
      });
    }) as never,
    location: {
      origin: "https://anydash.delino.io",
      pathname: "/settings",
      search: "",
    },
    history: {
      pushState: (_d, _u, url) => history.push(`push ${url}`),
      replaceState: (_d, _u, url) => history.push(`replace ${url}`),
    },
    clipboard: { writeText: async () => {} },
    open: (url) => opened.push(url),
    digest: async (text) => `digest(${text})`,
    savePath: (path) => saved.push(path),
    ...overrides,
  };
  return { value, requests, history, opened, saved };
}

describe("browser host", () => {
  it("adds only the AnyDash bearer token and allowed headers to API calls", async () => {
    const { value, requests } = deps();
    const host = new BrowserHost(value);
    const reply = JSON.parse(
      await host.reply(
        "rpc",
        JSON.stringify({
          target: "api",
          service: "anydash.v1.AccountService",
          method: "GetAccount",
          body: "AQID",
          headers: {
            "X-AnyDash-Client-Kind": "CLIENT_KIND_WEB",
            "X-AnyDash-Client-Version": "0.1.0",
            Cookie: "x",
          },
        }),
      ),
    );
    expect(reply).toEqual({
      ok: true,
      value: {
        status: 200,
        body: "AQID",
        contentType: "application/proto",
        correlationId: "corr-1",
      },
    });
    expect(requests[0].url).toBe(
      "https://anydash.api.delino.io/anydash.v1.AccountService/GetAccount",
    );
    const headers = requests[0].init.headers as Record<string, string>;
    expect(headers.Authorization).toBe(
      "Bearer token-for-https://anydash.api.delino.io",
    );
    expect(headers["X-AnyDash-Client-Kind"]).toBe("CLIENT_KIND_WEB");
    expect(headers.Cookie).toBeUndefined();
    expect(requests[0].init.credentials).toBe("omit");
  });
  it("returns the Delcore resource token separately", async () => {
    const host = new BrowserHost(deps().value);
    expect(await host.call("delcoreToken", {})).toEqual({
      token: "token-for-https://delcore.api.delino.io",
    });
  });
  it("refuses collector calls, non-HTTPS links and foreign services", async () => {
    const host = new BrowserHost(deps().value);
    expect(
      JSON.parse(
        await host.reply(
          "rpc",
          JSON.stringify({
            target: "collector",
            service: "anydash.v1.LocalCollectorService",
            method: "Quit",
          }),
        ),
      ).ok,
    ).toBe(false);
    expect(
      JSON.parse(
        await host.reply(
          "rpc",
          JSON.stringify({
            target: "api",
            service: "evil.Service",
            method: "X",
          }),
        ),
      ).ok,
    ).toBe(false);
    expect(
      JSON.parse(
        await host.reply(
          "openURL",
          JSON.stringify({ url: "http://example.com" }),
        ),
      ).ok,
    ).toBe(false);
  });
  it("saves the destination before signing in and manages history", async () => {
    const { value, saved, history } = deps();
    const host = new BrowserHost(value);
    await host.call("signIn", {});
    expect(saved).toEqual(["/settings"]);
    await host.call("route", { path: "/settings" });
    await host.call("route", { path: "/", replace: true });
    expect(history).toEqual(["push /settings", "replace /"]);
    expect(
      JSON.parse(
        await host.reply("route", JSON.stringify({ path: "//evil.example" })),
      ).ok,
    ).toBe(false);
    expect(CALLBACK_PATH).toBe("/callback");
  });
  it("derives an opaque user key and reports signed-out sessions", async () => {
    expect(await new BrowserHost(deps().value).call("session", {})).toEqual({
      signedIn: true,
      userKey: "digest(anydash:user-1)",
    });
    expect(
      await new BrowserHost(deps({ auth: undefined }).value).call(
        "session",
        {},
      ),
    ).toEqual({ signedIn: false });
  });
});
