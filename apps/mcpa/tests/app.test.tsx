// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, rs } from "@rstest/core";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { Code, ConnectError } from "@connectrpc/connect";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import type { QueryClient } from "@tanstack/react-query";
import {
  ErrorDetailSchema,
  ErrorReason,
  OauthFlowKind,
  OauthFlowState,
  Provider,
  ResourceRole,
} from "@repo/protos/mcpa/v1/mcpa_pb";
import { SESSION_QUERY_KEY } from "../src/app";
import { SessionStatus } from "../src/lib/session";
import {
  API,
  baseService,
  renderApp,
  RESOURCE_ID,
  resource,
  signedIn,
  TENANT_ID,
} from "./fixtures";

const resourcePath = (section = "") =>
  `/tenants/${TENANT_ID}/resources/${RESOURCE_ID}${section ? `/${section}` : ""}`;

/** Serializes every cached mutation's variables and data. */
const cachedMutationText = (queryClient: QueryClient) =>
  JSON.stringify(
    queryClient
      .getMutationCache()
      .getAll()
      .map((mutation) => [mutation.state.variables, mutation.state.data]),
    (_, value: unknown) =>
      typeof value === "bigint" ? value.toString() : value,
  );

const reasonError = (code: Code, reason: ErrorReason, url = "") =>
  new ConnectError("denied", code, undefined, [
    {
      desc: ErrorDetailSchema,
      value: { reason, traceId: "trace-9", reauthenticationUrl: url },
    },
  ]);

describe("session states", () => {
  it("shows sign-in with the exact console return URL when signed out", async () => {
    renderApp({
      path: "/tenants/tenant-1",
      sessions: [{ status: SessionStatus.SignedOut }],
    });
    const link = await screen.findByRole("link", { name: "Sign in" });
    expect(link.getAttribute("href")).toBe(
      `${API}/auth/login?return_to=${encodeURIComponent(`${window.location.origin}/`)}`,
    );
    // jsdom cannot navigate; the click handler still runs.
    link.addEventListener("click", (event) => event.preventDefault());
    fireEvent.click(link);
    expect(sessionStorage.getItem("mcpa.returnPath")).toBe("/tenants/tenant-1");
  });

  it("explains the MFA requirement and links to reauthentication", async () => {
    renderApp({
      sessions: [
        {
          status: SessionStatus.MfaRequired,
          reauthenticationUrl: "https://id.example/mfa",
        },
      ],
    });
    expect(
      await screen.findByRole("heading", {
        name: "Turn on two-step verification",
      }),
    ).toBeTruthy();
    expect(
      screen
        .getByRole("link", { name: "Set up two-step verification" })
        .getAttribute("href"),
    ).toBe("https://id.example/mfa");
  });

  it("ends the restricted session and signs in again after MFA setup", async () => {
    const opened: string[] = [];
    const { sessionClient } = renderApp({
      path: "/tenants/tenant-1",
      sessions: [
        {
          status: SessionStatus.MfaRequired,
          reauthenticationUrl: "https://id.example/mfa",
          csrfToken: "csrf-mfa",
        },
      ],
      openUrl: (url) => opened.push(url),
    });
    fireEvent.click(
      await screen.findByRole("button", {
        name: "I've done this. Sign in again",
      }),
    );
    await waitFor(() =>
      expect(opened).toEqual([
        `${API}/auth/login?return_to=${encodeURIComponent(`${window.location.origin}/`)}`,
      ]),
    );
    expect(sessionClient.calls).toEqual(["session", "logout:csrf-mfa"]);
    expect(sessionStorage.getItem("mcpa.returnPath")).toBe("/tenants/tenant-1");
  });

  it("shows the tenant picker for a signed-in user", async () => {
    renderApp();
    expect(await screen.findByRole("link", { name: /Acme/ })).toBeTruthy();
    expect(screen.getByRole("link", { name: /Ada \(personal\)/ })).toBeTruthy();
  });

  it("restores the page remembered before sign-in", async () => {
    sessionStorage.setItem("mcpa.returnPath", `/tenants/${TENANT_ID}/usage`);
    renderApp({
      service: {
        ...baseService(),
        getUsage: () => ({ summary: {}, rows: [] }),
      },
    });
    expect(
      await screen.findByRole("heading", { name: "Tenant usage" }),
    ).toBeTruthy();
    expect(window.location.pathname).toBe(`/tenants/${TENANT_ID}/usage`);
  });

  it("opens the remembered active tenant from the console root", async () => {
    localStorage.setItem("mcpa.activeTenant", TENANT_ID);
    renderApp();
    expect(
      await screen.findByRole("heading", { name: "Resources" }),
    ).toBeTruthy();
    expect(window.location.pathname).toBe(`/tenants/${TENANT_ID}`);
  });

  it("routes an MFA_REQUIRED Connect error to the MFA screen", async () => {
    const opened: string[] = [];
    const { sessionClient } = renderApp({
      service: {
        getViewer: () => {
          throw reasonError(
            Code.PermissionDenied,
            ErrorReason.MFA_REQUIRED,
            "https://id.example/mfa",
          );
        },
      },
      sessions: [
        signedIn,
        {
          status: SessionStatus.MfaRequired,
          reauthenticationUrl: "https://id.example/mfa",
          csrfToken: "csrf-mfa",
        },
      ],
      openUrl: (url) => opened.push(url),
    });
    expect(
      await screen.findByRole("heading", {
        name: "Turn on two-step verification",
      }),
    ).toBeTruthy();
    // The Connect error carries no CSRF token, so signing in again reads it
    // from the session before it ends the restricted session.
    fireEvent.click(
      screen.getByRole("button", { name: "I've done this. Sign in again" }),
    );
    await waitFor(() => expect(opened).toHaveLength(1));
    expect(sessionClient.calls).toEqual([
      "session",
      "session",
      "logout:csrf-mfa",
    ]);
  });

  it("returns to sign-in when the session can't be recovered", async () => {
    const { sessionClient } = renderApp({
      sessions: [
        {
          status: SessionStatus.SignedIn,
          userId: "user-1",
          csrfToken: "csrf-1",
        },
        { status: SessionStatus.SignedOut },
      ],
      service: {
        getViewer: () => {
          throw new ConnectError("expired", Code.Unauthenticated);
        },
      },
    });
    expect(await screen.findByRole("link", { name: "Sign in" })).toBeTruthy();
    expect(
      sessionClient.calls.filter((call) => call === "session"),
    ).toHaveLength(2);
  });

  it("signs out with the session client", async () => {
    const { sessionClient } = renderApp();
    fireEvent.click(await screen.findByRole("button", { name: "Sign out" }));
    expect(await screen.findByRole("link", { name: "Sign in" })).toBeTruthy();
    expect(sessionClient.calls).toContain("logout:csrf-1");
  });

  it("drops the previous user's data when a session refetch finds another user", async () => {
    let tenants = [{ id: TENANT_ID, name: "Acme", personal: false }];
    const { queryClient } = renderApp({
      sessions: [
        signedIn,
        {
          status: SessionStatus.SignedIn,
          userId: "user-2",
          csrfToken: "csrf-2",
        },
      ],
      service: {
        ...baseService(),
        getViewer: () => ({ viewer: { userId: "user", tenants } }),
      },
    });
    expect(await screen.findByRole("link", { name: /Acme/ })).toBeTruthy();
    // Another tab signed in as a different user; this tab finds the new
    // session when it refetches it.
    tenants = [{ id: "tenant-2", name: "Globex", personal: false }];
    await queryClient.invalidateQueries({ queryKey: SESSION_QUERY_KEY });
    expect(await screen.findByRole("link", { name: /Globex/ })).toBeTruthy();
    expect(screen.queryByRole("link", { name: /Acme/ })).toBeNull();
    expect(
      JSON.stringify(
        queryClient
          .getQueryCache()
          .getAll()
          .map((query) => query.state.data),
        (_, value: unknown) =>
          typeof value === "bigint" ? value.toString() : value,
      ),
    ).not.toContain("Acme");
  });
});

describe("dialogs", () => {
  it("closes with Esc and returns focus to the opener", async () => {
    renderApp({ path: `/tenants/${TENANT_ID}` });
    const opener = await screen.findByRole("button", {
      name: "Create resource",
    });
    opener.focus();
    fireEvent.click(opener);
    const dialog = await screen.findByRole("dialog");
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(opener));
  });

  it("focuses the name when the create form opens", async () => {
    renderApp({ path: `/tenants/${TENANT_ID}` });
    fireEvent.click(
      await screen.findByRole("button", { name: "Create resource" }),
    );
    const dialog = await screen.findByRole("dialog");
    await waitFor(() =>
      expect(document.activeElement).toBe(
        within(dialog).getByLabelText("Name"),
      ),
    );
  });

  it("creates a resource with a UUID v7 request key and opens it", async () => {
    const createResource = rs.fn(
      (request: { requestKey: string; name: string }) => ({
        resource: { ...resource(), name: request.name },
      }),
    );
    renderApp({
      path: `/tenants/${TENANT_ID}`,
      service: { ...baseService(), createResource },
    });
    fireEvent.click(
      await screen.findByRole("button", { name: "Create resource" }),
    );
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Name"), {
      target: { value: "Support bot" },
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Create resource" }),
    );
    await waitFor(() => expect(window.location.pathname).toBe(resourcePath()));
    expect(createResource.mock.calls[0][0].requestKey).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
  });

  it("explains a missing creation permission", async () => {
    renderApp({
      path: `/tenants/${TENANT_ID}`,
      service: {
        ...baseService(),
        createResource: () => {
          throw reasonError(
            Code.PermissionDenied,
            ErrorReason.PERMISSION_DENIED,
          );
        },
      },
    });
    fireEvent.click(
      await screen.findByRole("button", { name: "Create resource" }),
    );
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Name"), {
      target: { value: "Bot" },
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Create resource" }),
    );
    expect(
      await within(dialog).findByText(/permission to create resources/),
    ).toBeTruthy();
  });

  it("focuses the confirmation input when deleting a resource", async () => {
    renderApp({ path: resourcePath("settings") });
    fireEvent.click(
      await screen.findByRole("button", { name: "Delete resource" }),
    );
    const dialog = await screen.findByRole("dialog");
    const input = within(dialog).getByLabelText("Type Support bot to confirm");
    await waitFor(() => expect(document.activeElement).toBe(input));
    const confirm = within(dialog).getByRole("button", {
      name: "Delete resource",
    });
    expect((confirm as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(input, { target: { value: "Support bot" } });
    expect((confirm as HTMLButtonElement).disabled).toBe(false);
  });

  it("focuses the budget limit and saves a canonical amount", async () => {
    const setResourceBudget = rs.fn((request: { limitUsd: string }) => ({
      budget: { limitUsd: request.limitUsd, spentUsd: "0" },
    }));
    renderApp({
      path: resourcePath("budget"),
      service: {
        ...baseService(),
        getResourceBudget: () => ({
          budget: { limitUsd: "", spentUsd: "1.5" },
        }),
        setResourceBudget,
      },
    });
    fireEvent.click(await screen.findByRole("button", { name: "Set a limit" }));
    const dialog = await screen.findByRole("dialog");
    const input = within(dialog).getByLabelText("Monthly limit (USD)");
    await waitFor(() => expect(document.activeElement).toBe(input));
    fireEvent.change(input, { target: { value: "25.50" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save limit" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(setResourceBudget.mock.calls[0][0].limitUsd).toBe("25.5");
  });
});

describe("call keys", () => {
  it("shows the secret once and never again after closing", async () => {
    const secret =
      "mcpa_0192f0000000700080000000000000aa_secretsecretsecretsecretsecretsecretsec";
    let issued = false;
    const callKey = {
      id: "key-1",
      resourceId: RESOURCE_ID,
      name: "Backend",
      displayPrefix: "mcpa_0192f000…",
      issuerUserId: "user-1",
      createTime: timestampFromDate(new Date("2026-10-31T00:00:00Z")),
    };
    const { queryClient } = renderApp({
      path: resourcePath("keys"),
      service: {
        ...baseService(),
        issueCallKey: () => {
          issued = true;
          return { callKey, secret };
        },
        listCallKeys: () => ({ callKeys: issued ? [callKey] : [] }),
      },
    });
    fireEvent.click(
      (await screen.findAllByRole("button", { name: "Issue call key" }))[0],
    );
    const form = await screen.findByRole("dialog");
    const name = within(form).getByLabelText("Name");
    await waitFor(() => expect(document.activeElement).toBe(name));
    fireEvent.change(name, { target: { value: "Backend" } });
    fireEvent.click(within(form).getByRole("button", { name: "Issue key" }));
    const shown = await screen.findByRole("dialog", {
      name: "Copy your call key",
    });
    expect(
      (within(shown).getByLabelText("Call key") as HTMLInputElement).value,
    ).toBe(secret);
    expect(
      within(shown).getByText("You won't see this key again"),
    ).toBeTruthy();
    fireEvent.click(
      within(shown).getByRole("button", { name: "I've copied it" }),
    );
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(await screen.findByText("mcpa_0192f000…")).toBeTruthy();
    expect(document.body.innerHTML).not.toContain(secret);
    expect(cachedMutationText(queryClient)).not.toContain(secret);
  });

  it("drops the issued secret from the mutation cache when the page unmounts", async () => {
    const secret = "mcpa_unmount_secret_value";
    const { queryClient, unmount } = renderApp({
      path: resourcePath("keys"),
      service: {
        ...baseService(),
        issueCallKey: () => ({
          callKey: { id: "key-1", name: "Backend", displayPrefix: "mcpa_01…" },
          secret,
        }),
      },
    });
    fireEvent.click(
      (await screen.findAllByRole("button", { name: "Issue call key" }))[0],
    );
    const form = await screen.findByRole("dialog");
    fireEvent.change(within(form).getByLabelText("Name"), {
      target: { value: "Backend" },
    });
    fireEvent.click(within(form).getByRole("button", { name: "Issue key" }));
    await screen.findByRole("dialog", { name: "Copy your call key" });
    expect(cachedMutationText(queryClient)).toContain(secret);
    unmount();
    expect(cachedMutationText(queryClient)).not.toContain(secret);
  });

  it("resends the first expiry on retry and explains a replayed key", async () => {
    const issueCallKey = rs.fn(
      (request: { requestKey: string; expireTime?: { seconds: bigint } }) => {
        if (issueCallKey.mock.calls.length === 1)
          throw new ConnectError("lost", Code.Unavailable);
        // A replay of the committed request key returns no secret.
        return {
          callKey: {
            id: "key-1",
            resourceId: RESOURCE_ID,
            name: "Backend",
            displayPrefix: "mcpa_0192f000…",
            expireTime: request.expireTime,
          },
          secret: "",
        };
      },
    );
    const revokeCallKey = rs.fn(() => ({}));
    const now = rs.spyOn(Date, "now").mockReturnValue(1_800_000_000_000);
    try {
      renderApp({
        path: resourcePath("keys"),
        service: { ...baseService(), issueCallKey, revokeCallKey },
      });
      fireEvent.click(
        (await screen.findAllByRole("button", { name: "Issue call key" }))[0],
      );
      const form = await screen.findByRole("dialog");
      fireEvent.change(within(form).getByLabelText("Name"), {
        target: { value: "Backend" },
      });
      fireEvent.click(within(form).getByRole("button", { name: "Issue key" }));
      await waitFor(() => expect(issueCallKey).toHaveBeenCalledTimes(1));
      await within(form).findByRole("button", { name: "Issue key" });
      now.mockReturnValue(1_800_000_060_000);
      fireEvent.click(within(form).getByRole("button", { name: "Issue key" }));
      const created = await screen.findByRole("dialog", {
        name: "Your call key was created",
      });
      const [first, retry] = issueCallKey.mock.calls.map(
        ([request]) => request,
      );
      expect(retry.requestKey).toBe(first.requestKey);
      expect(retry.expireTime?.seconds).toBe(first.expireTime?.seconds);
      expect(first.expireTime?.seconds).toBe(
        BigInt((1_800_000_000_000 + 90 * 86_400_000) / 1000),
      );
      expect(within(created).queryByLabelText("Call key")).toBeNull();
      expect(within(created).getByText("mcpa_0192f000…")).toBeTruthy();
      fireEvent.click(
        within(created).getByRole("button", { name: "Revoke key" }),
      );
      const confirm = await screen.findByRole("dialog", {
        name: "Revoke Backend?",
      });
      fireEvent.click(
        within(confirm).getByRole("button", { name: "Revoke key" }),
      );
      await waitFor(() => expect(revokeCallKey).toHaveBeenCalledTimes(1));
      expect(revokeCallKey.mock.calls[0]).toBeDefined();
      const [revoked] = revokeCallKey.mock.calls[0] as unknown as [
        { callKeyId: string },
      ];
      expect(revoked.callKeyId).toBe("key-1");
    } finally {
      now.mockRestore();
    }
  });

  it("lists key status and asks before revoking", async () => {
    const revokeCallKey = rs.fn(() => ({}));
    renderApp({
      path: resourcePath("keys"),
      service: {
        ...baseService(),
        listCallKeys: () => ({
          callKeys: [
            {
              id: "k1",
              name: "Live",
              displayPrefix: "mcpa_aaaa…",
              issuerUserId: "user-1",
            },
            {
              id: "k2",
              name: "Old",
              displayPrefix: "mcpa_bbbb…",
              issuerUserId: "user-2",
              revokeTime: timestampFromDate(new Date("2026-10-01T00:00:00Z")),
            },
          ],
        }),
        revokeCallKey,
      },
    });
    expect(await screen.findByText("mcpa_aaaa…")).toBeTruthy();
    expect(screen.getAllByText("Never used")).toHaveLength(2);
    expect(screen.getByText("Revoked")).toBeTruthy();
    expect(screen.getAllByRole("button", { name: "Revoke" })).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Revoke" }));
    const dialog = await screen.findByRole("dialog", { name: "Revoke Live?" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Revoke key" }));
    await waitFor(() => expect(revokeCallKey).toHaveBeenCalledTimes(1));
  });
});

describe("role-based actions", () => {
  it("shows only invoke sections to invokers", async () => {
    renderApp({
      path: resourcePath(),
      service: baseService(ResourceRole.INVOKE),
    });
    const nav = await screen.findByRole("navigation", {
      name: "Resource sections",
    });
    const tabs = within(nav)
      .getAllByRole("link")
      .map((link) => link.textContent);
    expect(tabs).toEqual(["Endpoints", "Call keys", "Usage"]);
    expect(screen.getByText("Your role: Invoke")).toBeTruthy();
  });

  it("shows every section to managers", async () => {
    renderApp({ path: resourcePath() });
    const nav = await screen.findByRole("navigation", {
      name: "Resource sections",
    });
    expect(
      within(nav)
        .getAllByRole("link")
        .map((link) => link.textContent),
    ).toEqual([
      "Endpoints",
      "Call keys",
      "Credentials",
      "Routing",
      "Access",
      "Budget",
      "Usage",
      "Errors",
      "Audit log",
      "Settings",
    ]);
  });

  it("blocks manager-only sections for invokers opened by URL", async () => {
    const listCredentials = rs.fn(() => ({ credentials: [] }));
    renderApp({
      path: resourcePath("credentials"),
      service: { ...baseService(ResourceRole.INVOKE), listCredentials },
    });
    expect(await screen.findByText("Managers only")).toBeTruthy();
    expect(
      screen.queryByRole("button", { name: "Connect account" }),
    ).toBeNull();
    expect(listCredentials).not.toHaveBeenCalled();
  });
});

describe("views", () => {
  it("shows endpoint base URLs with placeholder-key examples", async () => {
    renderApp({ path: resourcePath() });
    expect(
      await screen.findByText(`https://mcpa.api.delino.io/r/${RESOURCE_ID}/v1`),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Copy OpenAI-compatible base URL" }),
    ).toBeTruthy();
    expect(document.body.textContent).toContain("$MCPA_KEY");
  });

  it("shows an empty state that explains the next step", async () => {
    renderApp({ path: resourcePath("credentials") });
    expect(await screen.findByText("No credentials yet")).toBeTruthy();
  });

  it("starts the resource list at the first page after a tenant switch", async () => {
    const PERSONAL_ID = "tenant-personal";
    const listResources = rs.fn(
      (request: { tenantId: string; page?: { afterId: string } }) => {
        const afterId = request.page?.afterId ?? "";
        if (request.tenantId === PERSONAL_ID)
          return {
            resources: afterId
              ? []
              : [{ ...resource(), id: "personal-1", name: "Personal bot" }],
          };
        return afterId
          ? { resources: [{ ...resource(), id: "acme-2", name: "Acme two" }] }
          : {
              resources: [{ ...resource(), id: "acme-1", name: "Acme one" }],
              nextAfterId: "acme-1",
            };
      },
    );
    renderApp({
      path: `/tenants/${TENANT_ID}`,
      service: { ...baseService(), listResources },
    });
    expect(await screen.findByText("Acme one")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByText("Acme two")).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Active tenant"), {
      target: { value: PERSONAL_ID },
    });
    expect(await screen.findByText("Personal bot")).toBeTruthy();
    expect(window.location.pathname).toBe(`/tenants/${PERSONAL_ID}`);
    const personalCalls = listResources.mock.calls
      .map(([request]) => request)
      .filter((request) => request.tenantId === PERSONAL_ID);
    expect(personalCalls.map((request) => request.page?.afterId ?? "")).toEqual(
      [""],
    );
  });

  it("shows a loading state while a request is pending", async () => {
    renderApp({
      path: resourcePath("audit"),
      service: {
        ...baseService(),
        listAuditEvents: () => new Promise(() => {}),
      },
    });
    expect(await screen.findByText("Loading audit log…")).toBeTruthy();
  });

  it("shows a safe error with a trace ID and retry", async () => {
    let attempts = 0;
    renderApp({
      path: resourcePath("audit"),
      service: {
        ...baseService(),
        listAuditEvents: () => {
          attempts += 1;
          throw reasonError(
            Code.Unavailable,
            ErrorReason.DEPENDENCY_UNAVAILABLE,
          );
        },
      },
    });
    expect(await screen.findByText("Temporarily unavailable")).toBeTruthy();
    expect(screen.getByText("Trace ID: trace-9")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(attempts).toBe(2));
  });

  it("handles a denied tenant budget gracefully", async () => {
    renderApp({
      path: `/tenants/${TENANT_ID}/budget`,
      service: {
        ...baseService(),
        getTenantBudget: () => {
          throw reasonError(
            Code.PermissionDenied,
            ErrorReason.PERMISSION_DENIED,
          );
        },
      },
    });
    expect(await screen.findByText("You can't view this budget")).toBeTruthy();
    expect(screen.getByText(/budget permission in Delcore/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Set a limit" })).toBeNull();
  });

  it("shows usage totals, the USD estimate and the final-bill note", async () => {
    const getUsage = rs.fn(() => ({
      summary: {
        callCount: 3n,
        requestBytes: 1n << 29n,
        responseBytes: 1n << 29n,
        totalBytes: 1n << 30n,
        totalGib: "1",
        estimatedUsd: "0.07",
      },
      rows: [
        {
          day: timestampFromDate(new Date("2026-10-31T00:00:00Z")),
          resourceId: RESOURCE_ID,
          model: "gpt-test",
          provider: Provider.CODEX,
          callCount: 3n,
          requestBytes: 1n << 29n,
          responseBytes: 1n << 29n,
        },
      ],
    }));
    renderApp({
      path: resourcePath("usage"),
      service: { ...baseService(), getUsage },
    });
    const summary = await screen.findByLabelText("Usage summary");
    expect(within(summary).getByText("1.000 GiB")).toBeTruthy();
    expect(within(summary).getByText("$0.07")).toBeTruthy();
    expect(screen.getByText(/Delcore issues the final bill/)).toBeTruthy();
    expect(screen.getByText("gpt-test")).toBeTruthy();
    const request = getUsage.mock.calls[0] as unknown as [
      { filter: { resourceIds: string[] } },
    ];
    expect(request[0].filter.resourceIds).toEqual([RESOURCE_ID]);
  });

  it("shows the last-manager rule when removal is rejected", async () => {
    renderApp({
      path: resourcePath("access"),
      service: {
        ...baseService(),
        listAclEntries: () => ({
          entries: [
            {
              resourceId: RESOURCE_ID,
              principalKind: 1,
              principalId: "user-1",
              role: ResourceRole.MANAGE,
            },
          ],
        }),
        removeAclEntry: () => {
          throw reasonError(
            Code.FailedPrecondition,
            ErrorReason.FAILED_PRECONDITION,
          );
        },
      },
    });
    fireEvent.click(await screen.findByRole("button", { name: "Remove" }));
    const dialog = await screen.findByRole("dialog", { name: "Remove You?" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    expect(
      await within(dialog).findByText("Keep at least one manager"),
    ).toBeTruthy();
  });
});

describe("OAuth connection", () => {
  it("completes an authorization-code flow with the pasted callback URL", async () => {
    const completeProviderOAuth = rs.fn((request: { callbackUrl: string }) => ({
      flow: {
        id: "flow-1",
        kind: OauthFlowKind.AUTHORIZATION_CODE,
        state: OauthFlowState.COMPLETED,
        credentialId: "cred-1",
      },
    }));
    renderApp({
      path: resourcePath("credentials"),
      service: {
        ...baseService(),
        startProviderOAuth: () => ({
          flow: {
            id: "flow-1",
            kind: OauthFlowKind.AUTHORIZATION_CODE,
            state: OauthFlowState.PENDING,
            provider: Provider.CODEX,
            authorizationUrl: "https://auth.example/authorize?x=1",
          },
        }),
        completeProviderOAuth,
      },
    });
    fireEvent.click(
      (await screen.findAllByRole("button", { name: "Connect account" }))[0],
    );
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Name"), {
      target: { value: "Team account" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Continue" }));
    const link = await within(dialog).findByRole("link", {
      name: /sign-in page/,
    });
    expect(link.getAttribute("href")).toBe(
      "https://auth.example/authorize?x=1",
    );
    const input = within(dialog).getByLabelText("Final page address");
    await waitFor(() => expect(document.activeElement).toBe(input));
    fireEvent.change(input, {
      target: { value: "http://localhost:1455/auth/callback?code=abc&state=s" },
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Finish connecting" }),
    );
    expect(await within(dialog).findByText("Account connected")).toBeTruthy();
    expect(completeProviderOAuth.mock.calls[0][0].callbackUrl).toBe(
      "http://localhost:1455/auth/callback?code=abc&state=s",
    );
  });

  it("shows the device code and polls until the flow completes", async () => {
    const getProviderOAuthFlow = rs.fn(() => ({
      flow: {
        id: "flow-2",
        kind: OauthFlowKind.DEVICE_CODE,
        state: OauthFlowState.COMPLETED,
      },
    }));
    renderApp({
      path: resourcePath("credentials"),
      service: {
        ...baseService(),
        startProviderOAuth: () => ({
          flow: {
            id: "flow-2",
            kind: OauthFlowKind.DEVICE_CODE,
            state: OauthFlowState.PENDING,
            userCode: "ABCD-1234",
            verificationUri: "https://device.example/activate",
          },
        }),
        getProviderOAuthFlow,
      },
    });
    fireEvent.click(
      (await screen.findAllByRole("button", { name: "Connect account" }))[0],
    );
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Name"), {
      target: { value: "Kimi" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Continue" }));
    expect(await within(dialog).findByText("ABCD-1234")).toBeTruthy();
    expect(await within(dialog).findByText("Account connected")).toBeTruthy();
    expect(getProviderOAuthFlow).toHaveBeenCalled();
  });

  it("offers a restart after a failed flow", async () => {
    renderApp({
      path: resourcePath("credentials"),
      service: {
        ...baseService(),
        startProviderOAuth: () => ({
          flow: {
            id: "flow-3",
            kind: OauthFlowKind.AUTHORIZATION_CODE,
            state: OauthFlowState.FAILED,
            failureMessage: "The provider rejected the request.",
          },
        }),
      },
    });
    fireEvent.click(
      (await screen.findAllByRole("button", { name: "Connect account" }))[0],
    );
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Name"), {
      target: { value: "A" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Continue" }));
    expect(
      await within(dialog).findByText("The provider rejected the request."),
    ).toBeTruthy();
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Start again" }),
    );
    expect(
      await within(dialog).findByRole("button", { name: "Continue" }),
    ).toBeTruthy();
  });

  it("clears the API key from the form after submission", async () => {
    const registerApiKeyCredential = rs.fn(() => {
      throw reasonError(Code.InvalidArgument, ErrorReason.INVALID_INPUT);
    });
    const { queryClient } = renderApp({
      path: resourcePath("credentials"),
      service: { ...baseService(), registerApiKeyCredential },
    });
    fireEvent.click(await screen.findByRole("button", { name: "Add API key" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Provider"), {
      target: { value: String(Provider.OPENAI_COMPATIBLE) },
    });
    fireEvent.change(within(dialog).getByLabelText("Name"), {
      target: { value: "Local" },
    });
    fireEvent.change(within(dialog).getByLabelText("API key"), {
      target: { value: "sk-secret" },
    });
    fireEvent.change(within(dialog).getByLabelText("Base URL"), {
      target: { value: "https://llm.example/v1" },
    });
    fireEvent.change(within(dialog).getByLabelText("Models"), {
      target: { value: "model-a, model-b\nmodel-a" },
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Add API key" }),
    );
    expect(
      await within(dialog).findByText("Check what you entered"),
    ).toBeTruthy();
    expect(
      (within(dialog).getByLabelText("API key") as HTMLInputElement).value,
    ).toBe("");
    expect(registerApiKeyCredential.mock.calls[0]).toBeDefined();
    const [request] = registerApiKeyCredential.mock.calls[0] as unknown as [
      { apiKey: string; models: string[]; baseUrl: string },
    ];
    expect(request.apiKey).toBe("sk-secret");
    expect(request.models).toEqual(["model-a", "model-b"]);
    expect(request.baseUrl).toBe("https://llm.example/v1");
    // The failed request's variables must not keep the key in the cache.
    expect(cachedMutationText(queryClient)).not.toContain("sk-secret");
    expect(
      within(dialog).getByText("For your security, enter the key again."),
    ).toBeTruthy();
  });

  it("ignores a hidden base URL after switching providers", async () => {
    const registerApiKeyCredential = rs.fn(() => ({
      credential: { id: "cred-1", name: "Prod" },
    }));
    renderApp({
      path: resourcePath("credentials"),
      service: { ...baseService(), registerApiKeyCredential },
    });
    fireEvent.click(await screen.findByRole("button", { name: "Add API key" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Provider"), {
      target: { value: String(Provider.OPENAI_COMPATIBLE) },
    });
    fireEvent.change(within(dialog).getByLabelText("Base URL"), {
      target: { value: "not a url" },
    });
    fireEvent.change(within(dialog).getByLabelText("Provider"), {
      target: { value: String(Provider.CODEX) },
    });
    expect(within(dialog).queryByLabelText("Base URL")).toBeNull();
    fireEvent.change(within(dialog).getByLabelText("Name"), {
      target: { value: "Prod" },
    });
    fireEvent.change(within(dialog).getByLabelText("API key"), {
      target: { value: "sk-openai" },
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Add API key" }),
    );
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(registerApiKeyCredential).toHaveBeenCalledTimes(1);
    const [request] = registerApiKeyCredential.mock.calls[0] as unknown as [
      { provider: Provider; baseUrl: string },
    ];
    expect(request.provider).toBe(Provider.CODEX);
    expect(request.baseUrl).toBe("");
  });

  it("drops the API key from the mutation cache after registration", async () => {
    const registerApiKeyCredential = rs.fn(() => ({
      credential: { id: "cred-1", name: "Prod" },
    }));
    const { queryClient } = renderApp({
      path: resourcePath("credentials"),
      service: { ...baseService(), registerApiKeyCredential },
    });
    fireEvent.click(await screen.findByRole("button", { name: "Add API key" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Name"), {
      target: { value: "Prod" },
    });
    fireEvent.change(within(dialog).getByLabelText("API key"), {
      target: { value: "sk-registered" },
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Add API key" }),
    );
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(registerApiKeyCredential).toHaveBeenCalledTimes(1);
    expect(cachedMutationText(queryClient)).not.toContain("sk-registered");
  });
});

describe("form validation matches the server", () => {
  const koreanName = "가".repeat(34); // 102 UTF-8 bytes, 34 UTF-16 units

  it("rejects a call-key name over 100 bytes without a request", async () => {
    const issueCallKey = rs.fn(() => ({}));
    renderApp({
      path: resourcePath("keys"),
      service: { ...baseService(), issueCallKey },
    });
    fireEvent.click(
      (await screen.findAllByRole("button", { name: "Issue call key" }))[0],
    );
    const form = await screen.findByRole("dialog");
    fireEvent.change(within(form).getByLabelText("Name"), {
      target: { value: koreanName },
    });
    fireEvent.click(within(form).getByRole("button", { name: "Issue key" }));
    expect(
      await within(form).findByText("Use a shorter name (up to 100 bytes)."),
    ).toBeTruthy();
    expect(issueCallKey).not.toHaveBeenCalled();
  });

  it("rejects a long resource name or description without a request", async () => {
    const createResource = rs.fn(() => ({ resource: resource() }));
    renderApp({
      path: `/tenants/${TENANT_ID}`,
      service: { ...baseService(), createResource },
    });
    fireEvent.click(
      await screen.findByRole("button", { name: "Create resource" }),
    );
    const dialog = await screen.findByRole("dialog");
    const submit = () =>
      fireEvent.click(
        within(dialog).getByRole("button", { name: "Create resource" }),
      );
    fireEvent.change(within(dialog).getByLabelText("Name"), {
      target: { value: koreanName },
    });
    submit();
    expect(
      await within(dialog).findByText("Use a shorter name (up to 100 bytes)."),
    ).toBeTruthy();
    fireEvent.change(within(dialog).getByLabelText("Name"), {
      target: { value: "Bot" },
    });
    fireEvent.change(within(dialog).getByLabelText("Description (optional)"), {
      target: { value: "가".repeat(334) }, // 1,002 bytes
    });
    submit();
    expect(
      await within(dialog).findByText(
        "Use a shorter description (up to 1,000 bytes).",
      ),
    ).toBeTruthy();
    expect(createResource).not.toHaveBeenCalled();
  });

  it("rejects invalid allowlist entries and accepts a valid one", async () => {
    renderApp({ path: resourcePath("routing") });
    const input = await screen.findByLabelText("Model name");
    const add = screen.getByRole("button", { name: "Add" });
    fireEvent.change(input, { target: { value: "my model" } });
    fireEvent.click(add);
    expect(
      await screen.findByText(
        "A model name can't contain spaces or control characters.",
      ),
    ).toBeTruthy();
    fireEvent.change(input, { target: { value: "a".repeat(201) } });
    fireEvent.click(add);
    expect(
      await screen.findByText("Use a shorter model name (up to 200 bytes)."),
    ).toBeTruthy();
    fireEvent.change(input, { target: { value: "gpt-4.1" } });
    fireEvent.click(add);
    expect(await screen.findByText("gpt-4.1")).toBeTruthy();
  });

  it("requires https for an OpenAI-compatible base URL", async () => {
    const registerApiKeyCredential = rs.fn(() => ({
      credential: { id: "cred-1", name: "Local" },
    }));
    renderApp({
      path: resourcePath("credentials"),
      service: { ...baseService(), registerApiKeyCredential },
    });
    fireEvent.click(await screen.findByRole("button", { name: "Add API key" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Provider"), {
      target: { value: String(Provider.OPENAI_COMPATIBLE) },
    });
    fireEvent.change(within(dialog).getByLabelText("Name"), {
      target: { value: "Local" },
    });
    fireEvent.change(within(dialog).getByLabelText("API key"), {
      target: { value: "sk-secret" },
    });
    fireEvent.change(within(dialog).getByLabelText("Base URL"), {
      target: { value: "http://api.example.com/v1" },
    });
    fireEvent.change(within(dialog).getByLabelText("Models"), {
      target: { value: "model-a" },
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Add API key" }),
    );
    expect(
      await within(dialog).findByText("Enter a URL starting with https://."),
    ).toBeTruthy();
    expect(registerApiKeyCredential).not.toHaveBeenCalled();
  });

  it("rejects a budget limit over 12 integer digits", async () => {
    const setResourceBudget = rs.fn(() => ({ budget: {} }));
    renderApp({
      path: resourcePath("budget"),
      service: {
        ...baseService(),
        getResourceBudget: () => ({ budget: { limitUsd: "", spentUsd: "0" } }),
        setResourceBudget,
      },
    });
    fireEvent.click(await screen.findByRole("button", { name: "Set a limit" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Monthly limit (USD)"), {
      target: { value: "1000000000000" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save limit" }));
    expect(
      await within(dialog).findByText("Enter an amount such as 25 or 25.50."),
    ).toBeTruthy();
    expect(setResourceBudget).not.toHaveBeenCalled();
  });
});

describe("budget display", () => {
  const meter = async () => (await screen.findByRole("meter")) as HTMLElement;

  it("shows a full meter for an exhausted zero limit", async () => {
    renderApp({
      path: resourcePath("budget"),
      service: {
        ...baseService(),
        getResourceBudget: () => ({
          budget: { limitUsd: "0", spentUsd: "0", exhausted: true },
        }),
      },
    });
    const element = await meter();
    expect(element.getAttribute("aria-valuenow")).toBe("100");
    const fill = element.querySelector("span") as HTMLElement;
    expect(fill.style.width).toBe("100%");
    expect(fill.hasAttribute("data-full")).toBe(true);
  });

  it("shows the spent share of the limit", async () => {
    renderApp({
      path: resourcePath("budget"),
      service: {
        ...baseService(),
        getResourceBudget: () => ({
          budget: { limitUsd: "10", spentUsd: "2.5" },
        }),
      },
    });
    const element = await meter();
    expect(element.getAttribute("aria-valuenow")).toBe("25");
    expect((element.querySelector("span") as HTMLElement).style.width).toBe(
      "25%",
    );
  });

  it("shows no meter without a limit", async () => {
    renderApp({
      path: resourcePath("budget"),
      service: {
        ...baseService(),
        getResourceBudget: () => ({ budget: { limitUsd: "", spentUsd: "1" } }),
      },
    });
    expect(await screen.findByText("No limit")).toBeTruthy();
    expect(screen.queryByRole("meter")).toBeNull();
  });

  it("saves an untouched prefilled limit unchanged even beyond typed-input limits", async () => {
    const setResourceBudget = rs.fn((request: { limitUsd: string }) => ({
      budget: { limitUsd: request.limitUsd, spentUsd: "0" },
    }));
    renderApp({
      path: resourcePath("budget"),
      service: {
        ...baseService(),
        getResourceBudget: () => ({
          budget: { limitUsd: "0.12345678", spentUsd: "0" },
        }),
        setResourceBudget,
      },
    });
    fireEvent.click(
      await screen.findByRole("button", { name: "Change limit" }),
    );
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Save limit" }));
    await waitFor(() => expect(setResourceBudget).toHaveBeenCalledTimes(1));
    expect(
      (setResourceBudget.mock.calls[0] as unknown as [{ limitUsd: string }])[0]
        .limitUsd,
    ).toBe("0.12345678");
  });

  it("saves a prefilled six-digit fraction limit unchanged", async () => {
    const setResourceBudget = rs.fn((request: { limitUsd: string }) => ({
      budget: { limitUsd: request.limitUsd, spentUsd: "0" },
    }));
    renderApp({
      path: resourcePath("budget"),
      service: {
        ...baseService(),
        getResourceBudget: () => ({
          budget: { limitUsd: "0.075", spentUsd: "0" },
        }),
        setResourceBudget,
      },
    });
    fireEvent.click(
      await screen.findByRole("button", { name: "Change limit" }),
    );
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Save limit" }));
    await waitFor(() => expect(setResourceBudget).toHaveBeenCalledTimes(1));
    expect(
      (setResourceBudget.mock.calls[0] as unknown as [{ limitUsd: string }])[0]
        .limitUsd,
    ).toBe("0.075");
  });
});
