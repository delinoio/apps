// SPDX-License-Identifier: Apache-2.0
// Test-only mocks. No production source imports this module.
import { render } from "@testing-library/react";
import { createRouterTransport, type ServiceImpl } from "@connectrpc/connect";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import {
  EndpointFormat,
  LoadBalancing,
  McpaService,
  ResourceRole,
} from "@repo/protos/mcpa/v1/mcpa_pb";
import { App, createQueryClient } from "../src/app";
import {
  SessionStatus,
  type Session,
  type SessionClient,
} from "../src/lib/session";

export const API = "https://api.test";
export const USER_ID = "user-1";
export const TENANT_ID = "tenant-1";
export const RESOURCE_ID = "0192f000-0000-7000-8000-000000000001";

export const signedIn: Session = {
  status: SessionStatus.SignedIn,
  userId: USER_ID,
  csrfToken: "csrf-1",
};

export function fakeSessionClient(...sessions: Session[]): SessionClient & {
  calls: string[];
} {
  const calls: string[] = [];
  let index = 0;
  return {
    calls,
    getSession: async () => {
      calls.push("session");
      const session = sessions[Math.min(index, sessions.length - 1)];
      index += 1;
      return session;
    },
    refresh: async () => {
      calls.push("refresh");
    },
    logout: async (csrfToken) => {
      calls.push(`logout:${csrfToken}`);
    },
    loginUrl: (returnTo) =>
      `${API}/auth/login?return_to=${encodeURIComponent(returnTo)}`,
  };
}

export const resource = (role = ResourceRole.MANAGE, name = "Support bot") => ({
  id: RESOURCE_ID,
  tenantId: TENANT_ID,
  name,
  description: "Answers support tickets",
  callerRole: role,
  createdByUserId: USER_ID,
  createTime: timestampFromDate(new Date("2026-10-01T00:00:00Z")),
  routing: { loadBalancing: LoadBalancing.ROUND_ROBIN, modelAllowlist: [] },
  endpoints: [
    {
      format: EndpointFormat.OPENAI,
      baseUrl: `https://mcpa.api.delino.io/r/${RESOURCE_ID}/v1`,
    },
    {
      format: EndpointFormat.CLAUDE,
      baseUrl: `https://mcpa.api.delino.io/r/${RESOURCE_ID}`,
    },
    {
      format: EndpointFormat.GEMINI,
      baseUrl: `https://mcpa.api.delino.io/r/${RESOURCE_ID}`,
    },
  ],
});

export const baseService = (
  role = ResourceRole.MANAGE,
): Partial<ServiceImpl<typeof McpaService>> => ({
  getViewer: () => ({
    viewer: {
      userId: USER_ID,
      tenants: [
        { id: TENANT_ID, name: "Acme", personal: false },
        { id: "tenant-personal", name: "Ada", personal: true },
      ],
    },
  }),
  listResources: () => ({ resources: [resource(role)] }),
  getResource: () => ({ resource: resource(role) }),
  listCallKeys: () => ({ callKeys: [] }),
  listCredentials: () => ({ credentials: [] }),
  listTenantGroups: () => ({ groups: [{ id: "group-1", name: "Engineers" }] }),
  listAclEntries: () => ({ entries: [] }),
});

export function renderApp({
  path = "/",
  sessions = [signedIn],
  service = baseService(),
  openUrl = () => {},
}: {
  path?: string;
  sessions?: Session[];
  service?: Partial<ServiceImpl<typeof McpaService>>;
  openUrl?: (url: string) => void;
} = {}) {
  window.history.replaceState(null, "", path);
  const sessionClient = fakeSessionClient(...sessions);
  const transport = createRouterTransport(({ service: register }) =>
    register(McpaService, service),
  );
  const queryClient = createQueryClient();
  queryClient.setDefaultOptions({
    queries: { retry: false, staleTime: Infinity, refetchOnWindowFocus: false },
  });
  const view = render(
    <App
      apiUrl={API}
      sessionClient={sessionClient}
      transport={transport}
      queryClient={queryClient}
      openUrl={openUrl}
    />,
  );
  return { ...view, sessionClient, queryClient };
}
