// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import { useQuery } from "@connectrpc/connect-query";
import { Button } from "@packages/ui/components/button";
import { NativeSelect } from "@packages/ui/components/native-select";
import type { Tenant } from "@repo/protos/mcpa/v1/mcpa_pb";
import { useSession } from "../app";
import { rpc } from "../lib/hooks";
import { Link, RouteName, paths, useRouter } from "../lib/router";
import { readActiveTenant, writeActiveTenant } from "../lib/tenant";
import { EmptyState } from "../components/states";
import { TenantPicker } from "./tenant-picker";
import { TenantPage } from "./tenant-page";
import { ResourcePage } from "./resource-page";

export function tenantName(tenant: { name: string; personal: boolean }) {
  return tenant.personal
    ? `${tenant.name || "Personal"} (personal)`
    : tenant.name;
}

export function sortTenants(tenants: Tenant[]): Tenant[] {
  return [...tenants].sort(
    (a, b) =>
      Number(b.personal) - Number(a.personal) || a.name.localeCompare(b.name),
  );
}

export function ConsoleShell() {
  const { route, navigate } = useRouter();
  const { signOut } = useSession();
  const viewer = useQuery(rpc.getViewer, {});
  const [signOutState, setSignOutState] = useState<"idle" | "busy" | "failed">(
    "idle",
  );
  const tenants = sortTenants(viewer.data?.viewer?.tenants ?? []);
  const tenantId =
    route.name === RouteName.Tenant || route.name === RouteName.Resource
      ? route.tenantId
      : undefined;
  const tenant = tenants.find((candidate) => candidate.id === tenantId);

  useEffect(() => {
    if (tenantId) writeActiveTenant(tenantId);
  }, [tenantId]);

  // Open the remembered tenant when the user returns to the console root.
  const [checkedHome, setCheckedHome] = useState(false);
  useEffect(() => {
    if (route.name !== RouteName.Home || checkedHome || !viewer.data) return;
    setCheckedHome(true);
    const remembered = readActiveTenant();
    if (tenants.some((candidate) => candidate.id === remembered))
      navigate(paths.tenant(remembered), { replace: true });
  }, [route.name, checkedHome, viewer.data, tenants, navigate]);

  return (
    <div className="app">
      <header className="topbar">
        <div className="topbar-inner">
          <Link to={paths.home()} className="brand">
            Managed CPA
          </Link>
          {tenants.length > 0 && (
            <label className="tenant-switcher">
              <span className="sr-only">Active tenant</span>
              <NativeSelect
                value={tenantId ?? ""}
                onChange={(event) =>
                  navigate(
                    event.target.value
                      ? paths.tenant(event.target.value)
                      : paths.home(),
                  )
                }
              >
                {!tenantId && <option value="">Choose a tenant</option>}
                {tenantId && !tenant && (
                  <option value={tenantId}>Unknown tenant</option>
                )}
                {tenants.map((candidate) => (
                  <option key={candidate.id} value={candidate.id}>
                    {tenantName(candidate)}
                  </option>
                ))}
              </NativeSelect>
            </label>
          )}
          <span className="spacer" />
          <Button
            variant="ghost"
            size="sm"
            disabled={signOutState === "busy"}
            onClick={async () => {
              setSignOutState("busy");
              try {
                await signOut();
              } catch {
                setSignOutState("failed");
              }
            }}
          >
            {signOutState === "busy" ? "Signing out…" : "Sign out"}
          </Button>
        </div>
        {signOutState === "failed" && (
          <p className="topbar-alert" role="alert">
            Sign-out didn't finish. Try again.
          </p>
        )}
      </header>
      <main className="content">
        {route.name === RouteName.Home && <TenantPicker viewer={viewer} />}
        {route.name === RouteName.Tenant && (
          <TenantPage
            tenantId={route.tenantId}
            tab={route.tab}
            tenant={tenant}
          />
        )}
        {route.name === RouteName.Resource && (
          <ResourcePage
            tenantId={route.tenantId}
            resourceId={route.resourceId}
            section={route.section}
          />
        )}
        {route.name === RouteName.NotFound && (
          <EmptyState
            title="Page not found"
            message="This address doesn't match a console page."
            action={
              <Button asChild>
                <Link to={paths.home()}>Go to tenants</Link>
              </Button>
            }
          />
        )}
      </main>
    </div>
  );
}
