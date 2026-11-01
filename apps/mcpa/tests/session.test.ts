// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, rs } from "@rstest/core";
import { createClient } from "@connectrpc/connect";
import { McpaService } from "@repo/protos/mcpa/v1/mcpa_pb";
import { resolveApiUrl } from "../src/lib/config";
import {
  consoleReturnUrl,
  createSessionClient,
  safeReturnPath,
  SessionStatus,
} from "../src/lib/session";
import { createMcpaTransport } from "../src/lib/transport";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

describe("API URL", () => {
  it("defaults to production only for production builds", () => {
    expect(resolveApiUrl({ PROD: true })).toBe("https://mcpa.api.delino.io");
    expect(resolveApiUrl({ PROD: false })).toBe("http://127.0.0.1:46329");
    expect(
      resolveApiUrl({
        PROD: true,
        PUBLIC_MCPA_API_URL: "https://mcpa-api.localhost/",
      }),
    ).toBe("https://mcpa-api.localhost");
  });
});

describe("session client", () => {
  it("reads signed-in, signed-out and MFA states", async () => {
    const fetchMock = rs
      .fn<typeof fetch>()
      .mockResolvedValueOnce(
        json(200, {
          user_id: "user-1",
          csrf_token: "csrf-1",
          expires_at: "2026-11-01T10:00:00Z",
        }),
      )
      .mockResolvedValueOnce(json(401, { error: "unauthenticated" }))
      .mockResolvedValueOnce(
        json(403, {
          error: "mfa_required",
          reauthentication_url: "https://id.example/mfa",
        }),
      );
    const client = createSessionClient("https://api.test", fetchMock);
    expect(await client.getSession()).toEqual({
      status: SessionStatus.SignedIn,
      userId: "user-1",
      csrfToken: "csrf-1",
      expiresAt: new Date("2026-11-01T10:00:00Z"),
    });
    expect(await client.getSession()).toEqual({
      status: SessionStatus.SignedOut,
    });
    expect(await client.getSession()).toEqual({
      status: SessionStatus.MfaRequired,
      reauthenticationUrl: "https://id.example/mfa",
    });
    expect(fetchMock.mock.calls[0][0]).toBe("https://api.test/auth/session");
    expect(fetchMock.mock.calls[0][1]?.credentials).toBe("include");
  });

  it("rejects unexpected responses and network failures", async () => {
    const client = createSessionClient(
      "https://api.test",
      rs
        .fn<typeof fetch>()
        .mockResolvedValueOnce(json(500, {}))
        .mockRejectedValueOnce(new TypeError("Failed to fetch")),
    );
    await expect(client.getSession()).rejects.toThrow();
    await expect(client.getSession()).rejects.toThrow();
  });

  it("sends the CSRF header on logout and refresh", async () => {
    const fetchMock = rs
      .fn<typeof fetch>()
      .mockImplementation(async () => new Response(null, { status: 204 }));
    const client = createSessionClient("https://api.test", fetchMock);
    await client.logout("csrf-1");
    await client.refresh("csrf-2");
    expect(
      fetchMock.mock.calls.map(([url, init]) => [url, init?.method]),
    ).toEqual([
      ["https://api.test/auth/logout", "POST"],
      ["https://api.test/auth/refresh", "POST"],
    ]);
    expect(
      new Headers(fetchMock.mock.calls[0][1]?.headers).get("X-Mcpa-CSRF"),
    ).toBe("csrf-1");
    expect(fetchMock.mock.calls[1][1]?.credentials).toBe("include");
  });

  it("builds the login URL with the exact console return URL", () => {
    const client = createSessionClient("https://api.test");
    expect(client.loginUrl(consoleReturnUrl("https://mcpa.delino.io"))).toBe(
      "https://api.test/auth/login?return_to=https%3A%2F%2Fmcpa.delino.io%2F",
    );
  });

  it("restores only same-origin paths after sign-in", () => {
    expect(safeReturnPath("/tenants/t1")).toBe("/tenants/t1");
    expect(safeReturnPath("//evil.example")).toBe("/");
    expect(safeReturnPath("/\\evil.example")).toBe("/");
    expect(safeReturnPath("https://evil.example")).toBe("/");
    expect(safeReturnPath(null)).toBe("/");
  });
});

describe("management transport", () => {
  const viewerResponse = () =>
    json(200, { viewer: { userId: "user-1", tenants: [] } });

  it("sends credentials and the CSRF header on every Connect call", async () => {
    const fetchMock = rs
      .fn<typeof fetch>()
      .mockImplementation(async () => viewerResponse());
    const transport = createMcpaTransport(
      "https://api.test",
      { csrfToken: () => "csrf-1", recover: async () => false },
      fetchMock,
    );
    const response = await createClient(McpaService, transport).getViewer({});
    expect(response.viewer?.userId).toBe("user-1");
    const [url, init] = fetchMock.mock.calls[0];
    expect(String(url)).toBe("https://api.test/mcpa.v1.McpaService/GetViewer");
    expect(init?.credentials).toBe("include");
    expect(new Headers(init?.headers).get("X-Mcpa-CSRF")).toBe("csrf-1");
  });

  it("refreshes the session and retries once after an authentication failure", async () => {
    let token = "old";
    const fetchMock = rs
      .fn<typeof fetch>()
      .mockResolvedValueOnce(
        json(401, { code: "unauthenticated", message: "expired" }),
      )
      .mockResolvedValueOnce(viewerResponse());
    const recover = rs.fn(async () => {
      token = "new";
      return true;
    });
    const transport = createMcpaTransport(
      "https://api.test",
      { csrfToken: () => token, recover },
      fetchMock,
    );
    await createClient(McpaService, transport).getViewer({});
    expect(recover).toHaveBeenCalledTimes(1);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(
      new Headers(fetchMock.mock.calls[1][1]?.headers).get("X-Mcpa-CSRF"),
    ).toBe("new");
  });

  it("does not retry when the session cannot be refreshed", async () => {
    const fetchMock = rs
      .fn<typeof fetch>()
      .mockImplementation(async () =>
        json(401, { code: "unauthenticated", message: "expired" }),
      );
    const transport = createMcpaTransport(
      "https://api.test",
      { csrfToken: () => "csrf", recover: async () => false },
      fetchMock,
    );
    await expect(
      createClient(McpaService, transport).getViewer({}),
    ).rejects.toThrow();
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("does not retry other failures", async () => {
    const recover = rs.fn(async () => true);
    const fetchMock = rs
      .fn<typeof fetch>()
      .mockImplementation(async () =>
        json(403, { code: "permission_denied", message: "no" }),
      );
    const transport = createMcpaTransport(
      "https://api.test",
      { csrfToken: () => "csrf", recover },
      fetchMock,
    );
    await expect(
      createClient(McpaService, transport).getViewer({}),
    ).rejects.toThrow();
    expect(recover).not.toHaveBeenCalled();
  });
});
