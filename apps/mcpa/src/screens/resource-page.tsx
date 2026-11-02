// SPDX-License-Identifier: Apache-2.0
import { useQuery } from "@connectrpc/connect-query";
import { Badge } from "@packages/ui/components/badge";
import { Button } from "@packages/ui/components/button";
import type { Resource } from "@repo/protos/mcpa/v1/mcpa_pb";
import { rpc } from "../lib/hooks";
import { describeError, ErrorKind } from "../lib/errors";
import { Link, paths } from "../lib/router";
import {
  canManage,
  Section,
  sectionLabels,
  sectionsForRole,
} from "../lib/roles";
import { roleLabel } from "../lib/labels";
import { EmptyState, ErrorState, Loading } from "../components/states";
import { TabNav } from "../components/tabs";
import { EndpointsView } from "../sections/endpoints";
import { KeysView } from "../sections/keys";
import { CredentialsView } from "../sections/credentials";
import { RoutingView } from "../sections/routing";
import { AccessView } from "../sections/access";
import { ResourceBudget } from "../sections/budget";
import { UsageView } from "../sections/usage";
import { ErrorsView } from "../sections/errors";
import { AuditView } from "../sections/audit";
import { SettingsView } from "../sections/settings";

export function ResourcePage({
  tenantId,
  resourceId,
  section,
}: {
  tenantId: string;
  resourceId: string;
  section: Section;
}) {
  const query = useQuery(rpc.getResource, { resourceId });
  const back = (
    <Link to={paths.tenant(tenantId)} className="back-link">
      ← All resources
    </Link>
  );
  if (query.isPending)
    return (
      <section className="page">
        {back}
        <Loading label="Loading resource…" />
      </section>
    );
  const resource = query.data?.resource;
  if (query.error || !resource) {
    const kind = query.error
      ? describeError(query.error).kind
      : ErrorKind.NotFound;
    return (
      <section className="page">
        {back}
        {kind === ErrorKind.NotFound || kind === ErrorKind.PermissionDenied ? (
          <EmptyState
            title="Resource unavailable"
            message="This resource doesn't exist or you don't have a role on it. Ask a resource manager for access."
            action={
              <Button asChild variant="outline">
                <Link to={paths.tenant(tenantId)}>Back to resources</Link>
              </Button>
            }
          />
        ) : (
          <ErrorState
            error={query.error}
            onRetry={() => void query.refetch()}
          />
        )}
      </section>
    );
  }
  const allowed = sectionsForRole(resource.callerRole);
  const visible = allowed.includes(section) ? section : undefined;
  return (
    <section className="page" aria-labelledby="resource-title">
      {back}
      <div className="resource-head">
        <h1 id="resource-title">{resource.name}</h1>
        <Badge
          variant="secondary"
          aria-label={`Your role: ${roleLabel(resource.callerRole)}`}
        >
          Your role: {roleLabel(resource.callerRole)}
        </Badge>
      </div>
      {resource.description && <p className="lead">{resource.description}</p>}
      <TabNav
        label="Resource sections"
        tabs={allowed}
        current={visible ?? Section.Endpoints}
        href={(next) => paths.resource(tenantId, resourceId, next)}
        names={sectionLabels}
      />
      {visible ? (
        <ResourceSection resource={resource} section={visible} />
      ) : (
        <EmptyState
          title="Managers only"
          message={`${sectionLabels[section]} is available to people with the Manage role on this resource. Ask a resource manager if you need it.`}
        />
      )}
    </section>
  );
}

function ResourceSection({
  resource,
  section,
}: {
  resource: Resource;
  section: Section;
}) {
  const manage = canManage(resource.callerRole);
  // Key names make usage and error rows readable. Invokers see their own keys.
  const keys = useQuery(
    rpc.listCallKeys,
    { resourceId: resource.id, page: { size: 200 } },
    { enabled: section === Section.Usage || section === Section.Errors },
  );
  switch (section) {
    case Section.Endpoints:
      return <EndpointsView resource={resource} />;
    case Section.Keys:
      return <KeysView resourceId={resource.id} role={resource.callerRole} />;
    case Section.Credentials:
      return <CredentialsView resourceId={resource.id} />;
    case Section.Routing:
      return (
        <RoutingView
          key={resource.updateTime?.seconds.toString()}
          resource={resource}
        />
      );
    case Section.Access:
      return (
        <AccessView resourceId={resource.id} tenantId={resource.tenantId} />
      );
    case Section.Budget:
      return <ResourceBudget resourceId={resource.id} />;
    case Section.Usage:
      return (
        <UsageView
          tenantId={resource.tenantId}
          resourceId={resource.id}
          callKeys={keys.data?.callKeys}
          description={
            manage
              ? undefined
              : "Measured traffic for model calls through this resource."
          }
        />
      );
    case Section.Errors:
      return (
        <ErrorsView
          tenantId={resource.tenantId}
          resourceId={resource.id}
          callKeys={keys.data?.callKeys}
        />
      );
    case Section.Audit:
      return <AuditView resourceId={resource.id} />;
    case Section.Settings:
      return (
        <SettingsView
          key={resource.updateTime?.seconds.toString()}
          resource={resource}
        />
      );
  }
}
