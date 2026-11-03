// SPDX-License-Identifier: Apache-2.0
import { useState, type FormEvent } from "react";
import { useMutation, useQuery } from "@connectrpc/connect-query";
import { Button } from "@packages/ui/components/button";
import { Input } from "@packages/ui/components/input";
import { Textarea } from "@packages/ui/components/textarea";
import { Badge } from "@packages/ui/components/badge";
import type { Tenant } from "@repo/protos/mcpa/v1/mcpa_pb";
import { rpc, useInvalidate } from "../lib/hooks";
import { Link, paths, TenantTab, useRouter } from "../lib/router";
import { useCursorPager } from "../lib/pager";
import { roleLabel } from "../lib/labels";
import { formatDateTime } from "../lib/format";
import { descriptionError, nameError } from "../lib/validate";
import { uuidv7 } from "../lib/uuid";
import { EmptyState, ErrorState, QueryView } from "../components/states";
import { Modal } from "../components/modal";
import { Field } from "../components/field";
import { PagerControls } from "../components/pager";
import { TabNav } from "../components/tabs";
import { UsageView } from "../sections/usage";
import { ErrorsView } from "../sections/errors";
import { TenantBudget } from "../sections/budget";
import { tenantName } from "./shell";

const tabNames: Record<TenantTab, string> = {
  [TenantTab.Resources]: "Resources",
  [TenantTab.Usage]: "Usage",
  [TenantTab.Errors]: "Errors",
  [TenantTab.Budget]: "Tenant budget",
};

export function TenantPage({
  tenantId,
  tab,
  tenant,
}: {
  tenantId: string;
  tab: TenantTab;
  tenant?: Tenant;
}) {
  return (
    <section className="page" aria-labelledby="tenant-title">
      <p className="eyebrow">Tenant</p>
      <h1 id="tenant-title">{tenant ? tenantName(tenant) : "Tenant"}</h1>
      <TabNav
        label="Tenant sections"
        tabs={Object.values(TenantTab)}
        current={tab}
        href={(next) => paths.tenant(tenantId, next)}
        names={tabNames}
      />
      {tab === TenantTab.Resources && <ResourceList tenantId={tenantId} />}
      {tab === TenantTab.Usage && <TenantUsage tenantId={tenantId} />}
      {tab === TenantTab.Errors && <TenantErrors tenantId={tenantId} />}
      {tab === TenantTab.Budget && <TenantBudget tenantId={tenantId} />}
    </section>
  );
}

function ResourceList({ tenantId }: { tenantId: string }) {
  const pager = useCursorPager();
  const [creating, setCreating] = useState(false);
  const resources = useQuery(rpc.listResources, {
    tenantId,
    page: { size: 50, afterId: pager.afterId },
  });
  return (
    <div className="section-body">
      <div className="section-head">
        <div>
          <h2>Resources</h2>
          <p className="muted">
            Each resource has its own endpoint, credentials, call keys and
            settings.
          </p>
        </div>
        <Button onClick={() => setCreating(true)}>Create resource</Button>
      </div>
      <QueryView query={resources} loadingLabel="Loading resources…">
        {(data) =>
          data.resources.length ? (
            <>
              <ul className="choice-list">
                {data.resources.map((resource) => (
                  <li key={resource.id}>
                    <Link
                      to={paths.resource(tenantId, resource.id)}
                      className="choice"
                    >
                      <span className="choice-title">
                        {resource.name}
                        <Badge variant="secondary">
                          {roleLabel(resource.callerRole)}
                        </Badge>
                      </span>
                      {resource.description && (
                        <span className="muted">{resource.description}</span>
                      )}
                      <span className="muted small">
                        Created {formatDateTime(resource.createTime)}
                      </span>
                    </Link>
                  </li>
                ))}
              </ul>
              <PagerControls pager={pager} nextAfterId={data.nextAfterId} />
            </>
          ) : (
            <EmptyState
              title="No resources yet"
              message="Create a resource to get an endpoint, connect provider accounts and issue call keys."
              action={
                <Button onClick={() => setCreating(true)}>
                  Create resource
                </Button>
              }
            />
          )
        }
      </QueryView>
      {creating && (
        <CreateResourceDialog
          tenantId={tenantId}
          onClose={() => setCreating(false)}
        />
      )}
    </div>
  );
}

function CreateResourceDialog({
  tenantId,
  onClose,
}: {
  tenantId: string;
  onClose: () => void;
}) {
  const { navigate } = useRouter();
  const invalidate = useInvalidate();
  // One idempotency key per opened form, so a retried submit is safe.
  const [requestKey] = useState(() => uuidv7());
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [touched, setTouched] = useState(false);
  const create = useMutation(rpc.createResource);
  const nameProblem = touched ? nameError(name) : undefined;
  const descriptionProblem = touched
    ? descriptionError(description)
    : undefined;
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setTouched(true);
    if (nameError(name) || descriptionError(description)) return;
    const response = await create
      .mutateAsync({
        requestKey,
        tenantId,
        name: name.trim(),
        description: description.trim(),
      })
      .catch(() => undefined);
    if (!response?.resource) return;
    await invalidate(rpc.listResources);
    onClose();
    navigate(paths.resource(tenantId, response.resource.id));
  };
  return (
    <Modal
      open
      onClose={onClose}
      title="Create a resource"
      description="You become its manager. You can rename it later."
      dismissible={!create.isPending}
    >
      <form className="form" onSubmit={submit} noValidate>
        <Field label="Name" error={nameProblem}>
          <Input
            autoFocus
            value={name}
            maxLength={100}
            onChange={(event) => setName(event.target.value)}
            placeholder="For example, Support assistant"
          />
        </Field>
        <Field label="Description (optional)" error={descriptionProblem}>
          <Textarea
            value={description}
            maxLength={1000}
            onChange={(event) => setDescription(event.target.value)}
          />
        </Field>
        {create.error && (
          <ErrorState
            compact
            error={create.error}
            permissionMessage="You need permission to create resources in this tenant. Ask a tenant admin in Delcore."
          />
        )}
        <div className="form-actions">
          <Button type="submit" disabled={create.isPending}>
            {create.isPending ? "Creating…" : "Create resource"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

/** Loads up to 200 resources for the resource filter of tenant views. */
function useFilterResources(tenantId: string) {
  return useQuery(rpc.listResources, { tenantId, page: { size: 200 } });
}

function TenantErrors({ tenantId }: { tenantId: string }) {
  const resources = useFilterResources(tenantId);
  return (
    <ErrorsView
      tenantId={tenantId}
      resources={resources.data?.resources ?? []}
    />
  );
}

function TenantUsage({ tenantId }: { tenantId: string }) {
  const resources = useFilterResources(tenantId);
  if (resources.error)
    return (
      <ErrorState
        error={resources.error}
        onRetry={() => void resources.refetch()}
      />
    );
  return (
    <UsageView
      tenantId={tenantId}
      resources={resources.data?.resources ?? []}
      title="Tenant usage"
      description="Usage across the resources you can access in this tenant."
    />
  );
}
