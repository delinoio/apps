// SPDX-License-Identifier: Apache-2.0
import type { UseQueryResult } from "@tanstack/react-query";
import type { GetViewerResponse } from "@repo/protos/mcpa/v1/mcpa_pb";
import { Link, paths } from "../lib/router";
import { EmptyState, QueryView } from "../components/states";
import { sortTenants, tenantName } from "./shell";

export function TenantPicker({
  viewer,
}: {
  viewer: UseQueryResult<GetViewerResponse>;
}) {
  return (
    <section aria-labelledby="tenant-picker-title" className="page narrow">
      <h1 id="tenant-picker-title">Choose a tenant</h1>
      <p className="lead">
        Resources, usage and billing belong to a Delcore tenant.
      </p>
      <QueryView query={viewer} loadingLabel="Loading your tenants…">
        {(data) => {
          const tenants = sortTenants(data.viewer?.tenants ?? []);
          if (!tenants.length)
            return (
              <EmptyState
                title="No tenants yet"
                message="Your account isn't a member of any Delcore tenant. Create an organization in Delcore or ask an admin to invite you."
              />
            );
          return (
            <ul className="choice-list">
              {tenants.map((tenant) => (
                <li key={tenant.id}>
                  <Link to={paths.tenant(tenant.id)} className="choice">
                    <span className="choice-title">{tenantName(tenant)}</span>
                    <span className="muted small">
                      {tenant.personal ? "Personal tenant" : "Organization"}
                    </span>
                  </Link>
                </li>
              ))}
            </ul>
          );
        }}
      </QueryView>
    </section>
  );
}
